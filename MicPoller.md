# MicPoller.md — El buffer de 960 frames del altavoz, `POLLERR` y la reconexión del stream de salida

> **Documento:** `MicPoller.md`
> **Componente:** stream de **salida** de voz (altavoz) en Linux — `cpal 0.15.3` → ALSA → PipeWire
> **Commit auditado:** `d8149a0` (`fix(voice): rebuild the voice session when the media path dies`), rama `build/unified-local-release`
> **Fecha:** 2026-10-28
> **Estado:** **propuesta / auditoría**. Nada de lo descrito en la sección 8 está implementado todavía.
> **Autor:** sesión de diagnóstico de voz (audio + reconexión)

---

## 0. Para qué sirve este documento

El 28/10 quedó arreglado lo que el usuario reportó explícitamente:

1. la sesión de voz ahora se reconstruye sola cuando la ruta UDP de medios muere (`94ed0d9` → `d8149a0`);
2. mute/deafen (botones y atajos) ya apuntan al canal y guild de la sesión de voz real, no al canal abierto en la UI;
3. el stream del altavoz ya no se recrea por cada ráfaga de `POLLERR` si los callbacks siguen vivos.

Lo que **no** se tocó, y es el motivo de este documento, es la causa de fondo que queda: el stream de salida se abre en Linux con un buffer ALSA muy pequeño (`BufferSize::Fixed(960)`, 20 ms *totales*), y cuando el sistema va cargado —por ejemplo decodificando el stream de vídeo de otra persona— ese buffer se queda corto y el PCM entra en estado de error (`alsa::poll() returned POLLERR`). En `cpal 0.15.3` ese estado **no se recupera solo**: el hilo de audio deja de pedir datos y la llamada queda muda hasta que Litecord destruye y recrea el stream (lo que ya hacemos en ~1 s con el watchdog nuevo).

Este documento reúne todo el contexto necesario para auditar una propuesta concreta: **dar más margen al buffer del altavoz, solo al altavoz, sin tocar el micrófono ni la ruta LTPV**.

---

## 1. Resumen ejecutivo

- **Qué se propone:** que el stream de salida en Linux use un buffer ALSA mayor (empezando por `Fixed(1920)` = 40 ms y terminando, si hace falta, en `Fixed(3840)` = 80 ms), manteniendo el micrófono exactamente como está (`Fixed(960)`).
- **Por qué:** el buffer actual son 20 ms con periodos efectivos de 5,33 ms (medido, ver §3.5). Cualquier pausa de planificación de más de 20 ms en el hilo de audio puede provocar un xrun y dejar el PCM en `POLLERR`, que cpal 0.15.3 no recupera.
- **Por qué solo el altavoz:** la ruta de micrófono/transmisión (captura → DSP → Opus 960 muestras → LTPV/DAVE) está validada por el usuario (“cuando se escucha, se escucha perfecto”) y no debe tocarse. Además, el buffer del micrófono no participa en el problema observado.
- **Cómo se validará:** primero instrumentación + override por variable de entorno (fase 0, sin cambio de comportamiento), luego A/B real con 960/1920/3840/Default midiendo `POLLERR` y huecos audibles (fase 1), y solo después se fija el valor ganador (fase 2).
- **Riesgo principal:** +10 a +40 ms de latencia de reproducción y la necesidad de ajustar el prebuffer de jitter (1920 muestras = 40 ms) si el callback pasa a 20 ms.
- **Criterio de aceptación:** 0 `POLLERR` (o reducción ≥ 99 %) en llamadas de 30 min con vídeo activo, sin huecos audibles, sin regresión en la transmisión.

---

## 2. Alcance y no-alcance

**Dentro del alcance**

- El stream de **salida** (`build_output_stream`) creado en `src/gateway/voice.rs` para reproducir las voces de los demás.
- La configuración ALSA que se le pasa (`linux_voice_stream_config(..., input = false)`).
- El prebuffer de jitter del altavoz y los umbrales que dependen del tamaño del callback.
- Instrumentación y plan de pruebas.

**Fuera del alcance (explícito)**

- El stream de **entrada** (`build_input_stream`) y su `Fixed(960)` (`src/audio/engine.rs:404`).
- El framing de 960 muestras del codificador Opus (`src/gateway/voice.rs:1600`, `1720`).
- DSP/AEC/HPF/denoiser y la cola de micrófono.
- La ruta de transmisión LTPV/DAVE y la negociación de cifrado.
- Windows y macOS (el cambio es `#[cfg(target_os = "linux")]`; en el resto se sigue usando el buffer por defecto del driver).
- El pipeline de vídeo (los `DECODER RX ... frame(s) descartado(s)` son otro problema, ver §5.4).

---

## 3. Cómo se construye hoy el stream del altavoz

### 3.1 Cadena de llamadas

```
connect_voice_gateway()                         src/gateway/voice.rs
 └─ hilo "speaker" (std::thread)
     ├─ host.output_devices() / default_output_device()
     ├─ select_preferred_48khz_config(&device, false)          src/audio/engine.rs:318
     ├─ linux_voice_stream_config(&device, &config, false)     src/audio/engine.rs:23  ← el 960
     └─ device.build_output_stream(&stream_config, callback, err_callback, None)
         └─ callback: mezcla las colas por SSRC, resample Hermite, soft-limit, AEC ref
```

Líneas relevantes (estado actual):

| Qué | Dónde |
|---|---|
| Constantes de colas / prebuffer | `src/gateway/voice.rs:24-26` |
| Config de salida 48 kHz | `src/gateway/voice.rs:1001` |
| Buffer Linux del altavoz | `src/gateway/voice.rs:1029` |
| Callback de mezcla (F32) | `src/gateway/voice.rs:1052` |
| Callback de mezcla (I16) | `src/gateway/voice.rs:1255` |
| Watchdog del stream | `src/gateway/voice.rs:1470-1500` |
| Constantes de stall | `src/gateway/voice.rs:2615-2618` |
| Helper del buffer | `src/audio/engine.rs:12-49` |
| Buffer del micrófono (no se toca) | `src/audio/engine.rs:404` |

### 3.2 Qué tamaño pide Litecord

```rust
// src/audio/engine.rs:12
fn voice_buffer_frames(rate: u32, supported: &cpal::SupportedBufferSize) -> u32 {
    // Nominal 20 ms, rounded to a 64-frame period before applying device limits.
    let frames = (rate / 50).max(128).div_ceil(64) * 64;
    match supported {
        cpal::SupportedBufferSize::Range { min, max } => frames.clamp(*min, *max),
        cpal::SupportedBufferSize::Unknown => frames,
    }
}
```

A 48 kHz: `48000 / 50 = 960` → ya es múltiplo de 64 → **960 frames** (20 ms). A 44,1 kHz: 896 (20,3 ms).

### 3.3 El “probe” solo comprueba que el stream se puede *crear*

```rust
// src/audio/engine.rs:23 (resumido)
let frames = voice_buffer_frames(...);
let mut fixed: cpal::StreamConfig = config.clone().into();
fixed.buffer_size = cpal::BufferSize::Fixed(frames);
let probe = device.build_output_stream(&fixed, |out| out.fill(0.0), |_| {}, None);
match probe {
    Ok(()) => { info!("Buffer de voz Linux aceito: {} frames (saída)", frames); fixed }
    Err(_) => { /* BufferSize::Default */ }
}
```

Puntos importantes para la auditoría:

- El probe **crea y destruye** un stream vacío. No ejecuta audio, no mide latencia, no detecta que el buffer sea demasiado justo: solo que ALSA acepte los parámetros.
- Por eso el log dice `Buffer de voz Linux aceito: 960 frames (saída)` incluso en las sesiones que después acumulan decenas de miles de `POLLERR`.
- El probe se repite en **cada** recreación del stream (cada vez que el watchdog reconstruye).

### 3.4 Qué hace cpal 0.15.3 exactamente con `Fixed(960)`

Versión resuelta en `Cargo.lock`: **cpal 0.15.3**. En `cpal-0.15.3/src/host/alsa/mod.rs` (función `init_hw_params`):

```rust
match config.buffer_size {
    BufferSize::Fixed(v) => {
        hw_params.set_period_size_near((v / 4) as alsa::pcm::Frames, alsa::ValueOr::Nearest)?;
        hw_params.set_buffer_size(v as alsa::pcm::Frames)?;
    }
    BufferSize::Default => {
        hw_params.set_period_time_near(25_000, alsa::ValueOr::Nearest)?;
        hw_params.set_buffer_time_near(100_000, alsa::ValueOr::Nearest)?;
    }
}
```

Dos consecuencias que no son obvias leyendo el código de Litecord:

1. **`Fixed(960)` no es un periodo de 960.** cpal pide `period_size = v / 4 = 240` frames y `buffer_size = 960` frames. Es decir: un anillo de **20 ms** dividido en periodos de **5 ms** (redondeados por ALSA).
2. **`BufferSize::Default` en Linux no es “lo que quiera el driver”**: cpal fija `period_time = 25 ms` y `buffer_time = 100 ms`.

### 3.5 Qué se midió en la práctica

En la sesión actual (`~/litecord_app.log`, 04:28-04:32 UTC), los contadores `SPK ... ticks=` del diagnóstico suben **937–939 por cada ventana de 5 s**:

```
937 ticks / 5 s = 187.5 callbacks/s
48000 Hz / 187.5 = 256 frames por callback (5.33 ms)
```

`256 = set_period_size_near(240, Nearest)` redondeado por ALSA/PipeWire a la potencia de dos más cercana. Esto encaja con el `node.latency = "240/48000"` que PipeWire mostraba para el sink-input de Litecord en una comprobación anterior con `pactl list sink-inputs` (5 ms): PipeWire adopta el periodo *pedido* como quantum del grafo y el plugin ALSA acaba entregando 256 frames.

Resumen del estado real del stream de salida:

| Parámetro | Pedido | Efectivo medido |
|---|---|---|
| Periodo (callback) | 240 frames (5 ms) | **≈ 256 frames (5,33 ms)** |
| Buffer total del anillo | 960 frames (20 ms) | 960 frames (20 ms) — 3,75 periodos |
| Callbacks por segundo | 200 | **≈ 187,5** |
| Margen ante un stall del hilo | 20 ms | 20 ms |

**Esto es el núcleo del asunto:** 20 ms de margen total. Cualquier cosa que retrase el hilo de audio más de 20 ms (un frame de vídeo H.264, un `mmap` grande, el recolector del asignador, un cambio de quantum de PipeWire) provoca un xrun.

---

## 4. No confundir tres “960” distintos

En el código hay tres números 960 y solo uno es el que se propone cambiar. Es la fuente de confusión más probable en la auditoría:

| # | Qué es | Dónde | ¿Se toca? |
|---|---|---|---|
| 1 | `BufferSize::Fixed(960)` del stream **de salida** (20 ms de anillo ALSA) | `voice.rs:1029` + `engine.rs:23` | **Sí (propuesta)** |
| 2 | `BufferSize::Fixed(960)` del stream **de entrada** (micrófono) | `engine.rs:404` | **No** |
| 3 | Frame de 960 muestras del codificador Opus (20 ms de audio) | `voice.rs:1554-1720` | **No** |

Además, el **prebuffer de jitter** (`VOICE_PREBUFFER_SAMPLES = 1920` muestras = 40 ms) es otra cosa: es la cantidad de audio decodificado que se acumula en la cola por SSRC antes de empezar a reproducir. No es el buffer de hardware, pero **interactúa** con el tamaño del callback (ver §8.4).

```
Red/DAVE → decodificador Opus (20 ms) → cola por SSRC (VOICE_QUEUE_LIMIT 0,6 s)
        → prebuffer 40 ms → callback ALSA (256 frames hoy) → sink PipeWire
```

---

## 5. Evidencia del problema

### 5.1 Números de las dos últimas sesiones

| Sesión | Ventana (UTC) | Duración | Líneas `alsa::poll() returned POLLERR` | “recriando o stream” | Ritmo |
|---|---|---|---|---|---|
| Anterior (`litecord_app.prev.log`) | 03:59:46 → 04:26:53 | 27 min 07 s | **49.456** | **75** | 1 corte cada ~22 s |
| Actual (`litecord_app.log`, con stall de 400 ms) | 04:28:34 → 04:32:49 | 4 min 15 s | **4** | **4** | 3 seguidos en 10 s (04:31) |

La sesión anterior es la que el usuario describe como “se escucha perfecto cuando se escucha”: el audio es correcto, pero cada ~22 s hay un corte de 1–2 s por recreación del stream.

### 5.2 Timeline del episodio 04:31 (sesión actual)

```
04:30:58.759  POLLERR (#1)
04:31:00.726  recriando o stream            ← 2 s de silencio (stall detectado)
04:31:02.522  Saída de Áudio: 48000Hz, 2 canais, F32
04:31:02.728  Buffer de voz Linux aceito: 960 frames (saída)
04:31:03.288  Stream ATIVO
04:31:03.536  POLLERR (#1)                  ← 0,25 s después de abrir
04:31:05.474  recriando o stream
04:31:08.013  Saída de Áudio ...
04:31:08.831  POLLERR (#1)                  ← 0,2 s después
04:31:10.853  recriando o stream
04:31:12.960  Saída de Áudio ...
04:31:13.016  Stream ATIVO                   ← a partir de aquí aguantó
```

Ese patrón —abrir, error a los 200–250 ms, recrear— es típico de un buffer que no admite el primer quantum real del grafo, no de un fallo de red ni de códec.

### 5.3 Por qué 49.456 líneas no son 49.456 cortes

En `cpal 0.15.3`, el worker del stream hace:

```rust
loop {
    let flow = poll_descriptors_and_prepare_buffer(...).unwrap_or_else(|err| {
        error_callback(err.into());       // ← nuestro warn "Erro no stream de saída"
        PollDescriptorsFlow::Continue     // ← y vuelve a poll inmediatamente
    });
    ...
}
```

Y `poll_descriptors_and_prepare_buffer` devuelve `POLLERR` *antes* de comprobar si hay datos:

```rust
let revents = stream.channel.revents(&descriptors[1..])?;
if revents.contains(alsa::poll::Flags::ERR) {
    return Err(BackendSpecificError { description: "`alsa::poll()` returned POLLERR" });
}
```

Con `timeout = None` (Litecord pasa `None`), `poll()` bloquea hasta que haya eventos; si el PCM está en error, `POLLERR` está siempre listo → **bucle cerrado a miles de iteraciones por segundo** hasta que alguien destruye el stream. De ahí las decenas de miles de líneas en pocos minutos: son ~600–800 iteraciones por cada corte, no 600 cortes.

### 5.4 Por qué después de `POLLERR` no vuelve el audio solo

En el mismo worker, cpal solo hace `prepare()` (la llamada de ALSA que saca el PCM del estado de error) cuando detecta **`XRun`**, y `XRun` solo se produce si `snd_pcm_avail()` devuelve `EPIPE`:

```rust
PollDescriptorsFlow::XRun => {
    if let Err(err) = stream.channel.prepare() { error_callback(err.into()); }
    continue;
}
...
let avail_frames = match stream.channel.avail() {
    Err(err) if err.errno() == libc::EPIPE => return Ok(PollDescriptorsFlow::XRun),
    res => res,
}?;
```

Pero cuando `revents` trae `POLLERR`, la función retorna **antes** de llegar a `avail()`. Resultado: `prepare()` nunca se llama, los data-callbacks no vuelven a ejecutarse y el stream queda mudo de forma permanente. **Por eso el watchdog tiene que destruir y recrear el stream**; no hay forma de “revivirlo” desde fuera con la API pública de cpal 0.15.3.

Esto también valida la decisión del commit `d8149a0`: no basta con ignorar `POLLERR`; hay que detectar el stall y reconstruir.

### 5.5 Contexto de carga en ese momento

En la misma ventana, el pipeline de vídeo estaba descartando frames continuamente:

```
04:30:50 → 04:31:15  ~10 líneas/s  [DECODER RX] 5..41 frame(s) descartado(s) ... Error { native: 16384 }
```

No es la causa directa del `POLLERR`, pero es el tipo de carga (decodificación + GPU + allocaciones) que retrasa el hilo de audio >20 ms y convierte un buffer justo en un xrun.

---

## 6. Hipótesis de causa raíz

Marcadas por nivel de confianza y con la forma de falsificarlas.

**H1 — El anillo de 20 ms es demasiado justo para el grafo de PipeWire (confianza alta).**
PipeWire trabaja con quanta de 5 ms (el periodo que pide cpal); el anillo completo son 20 ms = 4 quanta, y tras el redondeo a 256 quedan 3,75 periodos. Un cambio de quantum del grafo (otra app pidiendo menos/más latencia, cambio de sink, suspensión del nodo) puede dejar el anillo sin espacio → `POLLERR`.
*Falsificación:* con `Fixed(1920)` o `Fixed(3840)` el `POLLERR` debería desaparecer o caer >99 %. Si sigue igual, H1 es falsa.

**H2 — El hilo de audio no llega a tiempo bajo carga (confianza media-alta).**
El worker de cpal es un hilo normal (no RT) y el callback mezcla, resamplea y hace `Vec::with_capacity(frames)` + `push_speaker_reference` por invocación. Con 187 callbacks/s hay 187 oportunidades por segundo de llegar tarde; con 50 (callback de 20 ms) hay 50.
*Falsificación:* reproducir con el sistema ocioso (sin vídeo) → si no hay `POLLERR` en 30 min, apunta a carga; si hay, apunta a configuración.

**H3 — El nodo/sink de PipeWire se suspende o se reconfigura (confianza media).**
Si el sink entra en `SUSPENDED` (silencio prolongado) o cambia de perfil, el PCM de ALSA recibe `POLLERR`.
*Falsificación:* `pw-top`/`pactl` durante el fallo; si el nodo cambia de estado en el mismo instante, es H3.

**H4 — 960 no es múltiplo del periodo efectivo (256) (confianza baja, pero real).**
3,75 periodos por anillo; algunos plugins ALSA redondean el buffer o recalculan periodos en el primer `write`.
*Falsificación:* con `Fixed(3840)` el periodo pedido es 960 exacto y el anillo son 4 periodos exactos; si eso lo arregla, H4 y H1 se refuerzan mutuamente.

> **Nota:** las cuatro hipótesis comparten la misma intervención (más margen en el anillo). El A/B de §11 está diseñado para distinguirlas, no solo para “ver si mejora”.

---

## 7. Por qué no tocar el micrófono ni la ruta de transmisión

1. **Está validado por el usuario**: “cuando se escucha, se escucha perfecto”. Cualquier cambio en captura/DSP/Opus puede degradar algo que hoy funciona.
2. **El helper es compartido**: `linux_voice_stream_config` se usa con `input = true` en `engine.rs:404`. Cambiar la constante o la función sin separar la ruta tocaría el micrófono.
3. **El framing de TX no depende del periodo del micrófono**: el callback de captura entrega `data.len()/channels` muestras y las empuja a la cola; el codificador consume **exactamente 960** muestras por frame (`voice.rs:1600-1720`). Es decir, el buffer del micrófono es independiente del framing LTPV: no hay ninguna razón técnica para moverlo.
4. **El “poll” del micrófono no aparece en los logs de fallo**: los `POLLERR` observados son del stream de salida (`Erro no stream de saída F32`). No hay evidencia de fallo equivalente en entrada.

---

## 8. Propuesta

### 8.1 Fase 0 — Instrumentación y override (sin cambiar comportamiento)

Objetivo: poder medir y comparar **sin recompilar** y sin alterar el comportamiento por defecto.

1. Nuevo helper solo para salida:
   ```rust
   // src/audio/engine.rs
   #[cfg(target_os = "linux")]
   pub(crate) fn linux_voice_output_config(
       device: &cpal::Device,
       config: &cpal::SupportedStreamConfig,
   ) -> cpal::StreamConfig {
       let frames = std::env::var("LITECORD_VOICE_OUT_FRAMES")
           .ok()
           .and_then(|v| v.parse::<u32>().ok())
           .unwrap_or(960);          // ← default: comportamiento actual, idéntico
       voice_config_with_frames(device, config, frames, false)
   }
   ```
   `voice_config_with_frames` es el cuerpo actual de `linux_voice_stream_config` parametrizado por `frames` (misma lógica de clamp + probe + fallback a `Default`).

2. `linux_voice_stream_config(..., input = true)` **no se toca** (el micrófono sigue con 960).

3. En el callback, un log único por stream (para saber qué negoció ALSA de verdad):
   ```rust
   if ticks_f32.load(Relaxed) == 1 {
       let frames = output.len() / out_channels.max(1);
       info!("🔊 Speaker callback: {} frames ({:.2} ms) @ {} Hz",
             frames, frames as f64 * 1000.0 / out_sample_rate as f64, out_sample_rate);
   }
   ```
   (mismo añadido en la rama I16).

4. Reutilizar el contador `output_err_count` que ya existe para reportar en el diagnóstico `📊 [VOICE DIAG]` el total de `POLLERR` de la sesión (hoy solo se usa para limitar el log). Así el A/B se lee de una línea.

**Resultado esperado de la fase 0:** cero cambios audibles y cero cambios de comportamiento; solo datos.

### 8.2 Fase 1 — A/B en llamadas reales

Cuatro valores, mismo binario, solo reiniciando con distinta variable:

| Variante | `LITECORD_VOICE_OUT_FRAMES` | Periodo pedido | Anillo | Callback esperado | Latencia añadida aprox. |
|---|---|---|---|---|---|
| A (actual) | `960` (o sin var) | 240 → 256 | 20 ms | 5,33 ms | — |
| B | `1920` | 480 | 40 ms | 10 ms | +10–20 ms |
| C | `3840` | 960 | 80 ms | 20 ms | +30–40 ms |
| D | `0` (= `Default`) | 25 ms | 100 ms | ~25 ms | +40–60 ms |

Protocolo por variante: una llamada de ≥ 15 min con alguien hablando y un stream de vídeo reproduciéndose (la carga que dispara el fallo), y anotar:

- `grep -c 'alsa::poll' ~/litecord_app.log`
- `grep -c 'recriando o stream' ~/litecord_app.log`
- el log nuevo `🔊 Speaker callback: N frames`
- `grep 'Erro no stream de saída' | wc -l`
- percepción del usuario (huecos, latencia, “se siente raro”).

### 8.3 Fase 2 — Fijar el ganador

Según el resultado, dejar como constante el valor ganador (probablemente 1920 si basta, 3840 si no) y retirar o mantener la variable de entorno como escape de diagnóstico.

### 8.4 Ajuste obligatorio del prebuffer si el callback crece

Si el callback pasa a 20 ms (variante C/D), el prebuffer actual de **40 ms** son solo 2 callbacks. Si el decodificador se retrasa >40 ms, la cola se vacía a mitad de callback y el código aplica el camino de “decay” (fade), lo que se oiría como microcortes aunque el dispositivo esté sano.

Propuesta: prebuffer proporcional al callback, **solo en Linux** para no cambiar Windows:

```rust
// dentro del callback, después de calcular `frames`
#[cfg(target_os = "linux")]
let prebuffer = VOICE_PREBUFFER_SAMPLES.max(frames * 3);   // 3 callbacks de margen
#[cfg(not(target_os = "linux"))]
let prebuffer = VOICE_PREBUFFER_SAMPLES;                    // sin cambios
...
} else if q.len() >= prebuffer {
```

Con `frames = 256` (hoy): `max(1920, 768) = 1920` → **sin cambio**.
Con `frames = 960` (80 ms): `max(1920, 2880) = 2880` (60 ms) → margen correcto.

`VOICE_RESET_INACTIVE_CALLBACKS = 3` también es sensible al tamaño del callback (3 callbacks = 15 ms hoy; 60 ms con 20 ms). No es crítico, pero conviene revisarlo en la fase 2 si el inicio de habla se siente perezoso.

### 8.5 Qué NO cambia en la propuesta

- Micrófono: mismo `Fixed(960)`, mismo callback, misma cola.
- Opus/TX: mismo framing de 960 muestras, mismo `pcm_frame`, mismos timestamps.
- DSP/AEC: `push_speaker_reference` recibe lotes más grandes, pero el anillo es agnóstico al tamaño (comprobado en `src/audio/dsp.rs:125`).
- Windows/macOS: mismo `config.clone().into()` (BufferSize::Default del sistema).
- Watchdog: sigue igual; el umbral de 400 ms tras error y el stall de 2 s no dependen del tamaño del callback.

---

## 9. Diff propuesto (fase 0 + fase 2, resumido)

```diff
--- a/src/audio/engine.rs
+++ b/src/audio/engine.rs
@@
 pub(crate) fn linux_voice_stream_config(
     device: &cpal::Device, config: &cpal::SupportedStreamConfig, input: bool,
 ) -> cpal::StreamConfig {
+    // Entrada: sin cambios (960). Salida: margen configurable.
+    let frames = if input {
+        voice_buffer_frames(config.sample_rate().0, config.buffer_size())
+    } else {
+        voice_output_frames()
+    };
+    voice_config_with_frames(device, config, frames, input)
 }
+
+#[cfg(target_os = "linux")]
+fn voice_output_frames() -> u32 {
+    std::env::var("LITECORD_VOICE_OUT_FRAMES").ok()
+        .and_then(|v| v.parse::<u32>().ok())
+        .filter(|v| *v >= 256)
+        .unwrap_or(960)          // fase 0: idéntico a hoy; fase 2: valor ganador
+}
```

```diff
--- a/src/gateway/voice.rs
+++ b/src/gateway/voice.rs
@@
                                                             device.build_output_stream(
                                                                 &stream_config,
                                                                 move |output: &mut [f32], _| {
                                                                     ticks_f32.fetch_add(1, Ordering::Relaxed);
+                                                                    if ticks_f32.load(Ordering::Relaxed) == 1 {
+                                                                        let f = output.len() / out_channels.max(1);
+                                                                        info!("🔊 Speaker callback: {} frames ({:.2} ms) @ {} Hz",
+                                                                            f, f as f64 * 1000.0 / out_sample_rate as f64, out_sample_rate);
+                                                                    }
```

Nota: el diff exacto debe respetar la indentación real del callback (está anidado muy profundo). Se recomienda extraer `voice_config_with_frames` sin cambiar la firma pública de `linux_voice_stream_config`, para que el micrófono y el loopback de test mic sigan usando exactamente el mismo camino.

---

## 10. Efectos secundarios y mitigaciones

| Efecto | Magnitud | Mitigación |
|---|---|---|
| Latencia de reproducción | +10–40 ms según variante | Empezar por 1920; 3840 solo si 1920 no basta |
| Menos callbacks/s | 187 → 94 (1920) → 47 (3840) | Menos CPU y menos probabilidad de llegar tarde (positivo) |
| Prebuffer insuficiente con callbacks grandes | Vacíos a mitad de callback | Prebuffer dinámico Linux-only (§8.4) |
| `aec_ref` más grande por callback | +3,8 KB por 20 ms | Aceptable; el anillo AEC ya está dimensionado (§8.5) |
| Inicio de habla más perezoso | +15–40 ms | Revisar `VOICE_RESET_INACTIVE_CALLBACKS` si se nota |
| El probe acepta y luego falla | Igual que hoy | El watchdog ya reconstruye; la fase 0 no lo cambia |
| Dispositivo que rechaza 3840 | Fallback a `Default` | Ya implementado en el probe (clamp + fallback) |

---

## 11. Plan de pruebas y criterios de aceptación

### 11.1 Escenarios

1. **Llamada en silencio** 15 min (nadie habla) → no debe haber `POLLERR` ni recreaciones.
2. **Conversación normal** 15 min → sin cortes; comprobar prebuffer (inicio de habla).
3. **Vídeo + voz** 15 min (el escenario que dispara el fallo) → métrica principal.
4. **Cambio de dispositivo de salida** en caliente → el stream se recrea sin error y sin doble audio.
5. **Deafen/undeafen** en mitad de reproducción → sin backlog ni silencio permanente.
6. **Corte de red** (si se puede simular: apagar Wi-Fi 10 s) → reconexión de la sesión de voz (esto es del commit `d8149a0`, se prueba a la vez).

### 11.2 Criterios de aceptación

- Variante ganadora: **0** `POLLERR` o reducción ≥ 99 % respecto a la línea base (49.456 en 27 min).
- 0 recreaciones del stream por `POLLERR` en 30 min de escenario 3.
- Sin huecos audibles reportados por el usuario.
- Latencia percibida aceptable (no debe notarse “eco” ni retraso al hablar).
- Transmisión intacta: `frames_enviados` y `peak` del micrófono siguen normales; sin cambios en `mic_queue`.

### 11.3 Comandos útiles durante la auditoría

```bash
# métricas del log que ya escribe la app
grep -c 'alsa::poll'            ~/litecord_app.log
grep -c 'recriando o stream'    ~/litecord_app.log
grep    'Speaker callback'      ~/litecord_app.log | tail -5
grep    'VOICE DIAG'            ~/litecord_app.log | tail -3

# estado real del nodo de audio mientras hay llamada
pactl list sink-inputs | grep -E -A2 -B12 'litecord|Litecord'
pactl list sink-inputs | grep node.latency
pw-top -b -n 1 | grep -i lite

# carga del sistema en el momento del fallo
uptime
ps -o pid,pcpu,pmem,comm -C litecord
```

---

## 12. Rollback

- Fase 0: `LITECORD_VOICE_OUT_FRAMES=960` (o ausencia de la variable) reproduce el comportamiento actual exacto; el log nuevo es inocuo.
- Fase 2: volver la constante a 960 y el prebuffer a `VOICE_PREBUFFER_SAMPLES`; un solo commit.
- El watchdog y las correcciones de mute/reconnect son independientes y no se revierten con esto.

---

## 13. Alternativas consideradas y descartadas

| Alternativa | Por qué no (por ahora) |
|---|---|
| `BufferSize::Default` en el altavoz | Es la variante D del A/B, pero deja el tamaño en manos de PipeWire (100 ms fijos en cpal 0.15.3). Se medirá, no se adopta a ciegas. |
| Backend nativo PipeWire/Pulse (sin ALSA) | Cambio de dependencia y de toda la ruta de audio; fuera de alcance y con riesgo alto para LTPV. |
| Hilo de audio con prioridad RT (`SCHED_FIFO`) | Reduce H2 pero no H1/H3; requiere privilegios y es más invasivo que subir el buffer. Se puede evaluar después. |
| Llamar a `snd_pcm_prepare()` desde el error callback | cpal 0.15.3 no expone el handle ALSA; habría que parchear/vendorizar cpal. Descartado. |
| Bajar el quantum de PipeWire a 256 fijo para Litecord | Requiere `PIPEWIRE_NODE`/reglas del servidor y afecta a otras apps; no es portable. |
| No hacer nada y confiar en el rebuild de 1 s | Es la línea base actual: 1 corte cada ~22 s con vídeo. Aceptable como red de seguridad, no como solución. |

---

## 14. Checklist de auditoría (preguntas concretas)

1. ¿La afirmación “`Fixed(960)` produce periodo 240 y buffer 960” coincide con el cpal 0.15.3 del `Cargo.lock`? (§3.4)
2. ¿La medición de 187,5 callbacks/s → 256 frames es sólida, o puede el contador `ticks` contar otra cosa? (§3.5)
3. ¿El `POLLERR` de cpal 0.15.3 realmente no llama a `prepare()`? (§5.4)
4. ¿El prebuffer de 40 ms es suficiente con un callback de 20 ms? (§8.4)
5. ¿Hay algún sitio más que dependa del tamaño del callback y no esté listado en §8.5? (buscar `output.len()`, `frames`, `aec_ref`, `voice_played_*`)
6. ¿El cambio debe ser `cfg(target_os = "linux")` también en el prebuffer? (§8.4)
7. ¿La latencia añadida es aceptable para el uso del usuario (voz + música/streams)?
8. ¿El A/B necesita más de una sesión por variante para descartar el azar? (recomendado: 2 sesiones de 15 min por variante)
9. ¿El log `Speaker callback` debe ir en ambas ramas (F32/I16) y en el loopback de test mic? (solo salida de voz, ambas ramas)
10. ¿La variable de entorno debe quedar documentada en el README o solo en este documento?

---

## 15. Anexos

### 15.1 Extractos de código citados

`src/audio/engine.rs:12-19` — tamaño nominal:

```rust
fn voice_buffer_frames(rate: u32, supported: &cpal::SupportedBufferSize) -> u32 {
    let frames = (rate / 50).max(128).div_ceil(64) * 64;
    ...
}
```

`src/audio/engine.rs:23-49` — probe + fallback (resumido arriba en §3.3).

`src/gateway/voice.rs:24-26` — colas y prebuffer:

```rust
const VOICE_QUEUE_LIMIT: usize = 28800;          // 0,6 s por SSRC
const VOICE_PREBUFFER_SAMPLES: usize = 1920;     // 40 ms
const VOICE_RESET_INACTIVE_CALLBACKS: u32 = 3;
```

`src/gateway/voice.rs:1470-1500` — watchdog del stream (estado actual):

```rust
let stalled = quiet_since.elapsed();
if stalled >= OUTPUT_STALL || (broken && stalled >= OUTPUT_STALL_AFTER_ERROR) {
    warn!("🔊 Saída de áudio {}: recriando o stream.", if broken { "com erro" } else { "parada" });
    ...
    drop(stream);
    break;   // vuelve a abrir el stream
}
```

Con `OUTPUT_STALL = 2 s` y `OUTPUT_STALL_AFTER_ERROR = 400 ms` (`voice.rs:2615-2618`).

### 15.2 Historial relevante

| Commit | Qué hizo | Relación con este documento |
|---|---|---|
| `4468aaa` | Introduce `BufferSize::Fixed(20 ms)` en Linux (mic) | Origen del objetivo de 20 ms |
| `7e680ff` | Alinea a múltiplos de 64 y añade probe con fallback (salida) | Origen del probe |
| `31a66dc` | Unifica en `linux_voice_stream_config` para mic **y** altavoz | Origen del 960 compartido |
| `e9aa59f` | Conserva voz baja y acota el audio en cola | Toca colas/prebuffer |
| `94ed0d9`/`d8149a0` | Reconexión de sesión + stall de salida + mute/deafen correctos | Estado actual |

### 15.3 Glosario

- **Periodo / callback**: bloque de `N` frames que ALSA pide de golpe. `N/48000` = ms por callback.
- **Anillo (buffer)**: memoria total del PCM; si se llena o se vacía → xrun.
- **XRun**: underrun/overrun. cpal lo detecta por `EPIPE` en `avail()` y hace `prepare()`.
- **POLLERR**: el descriptor del PCM reporta error en `poll()`; cpal lo trata como error del backend y **no** hace `prepare()`.
- **Quantum**: bloque de proceso de PipeWire (hoy 5 ms para este stream, derivado del periodo pedido).
- **Prebuffer de jitter**: audio decodificado acumulado antes de empezar a sonar; absorbe jitter de red, no de hardware.

---

## 16. Decisión pendiente

El documento no aplica cambios. Para avanzar hace falta una de estas tres decisiones:

1. **Aprobar fase 0** (instrumentación + override) y medir el A/B con el usuario.
2. **Ir directo a la variante B** (`Fixed(1920)` + prebuffer dinámico) aceptando +10–20 ms de latencia.
3. **Dejarlo como está** y convivir con los cortes ocasionales, confiando en el rebuild de ~1 s.

La recomendación del autor es la **opción 1**: es reversible, no cambia el audio por defecto y produce la evidencia que falta para elegir entre B, C y D sin adivinar.
