> **Estado 2026-11-03:** varias secciones de este documento ya están
> implementadas y verificadas. Consulta
> [`ROADMAP_2026-11-03_CIERRE.md`](ROADMAP_2026-11-03_CIERRE.md) para el estado
> vivo, lo implementado y lo que queda pendiente.

# Litecord — Roadmap de mejoras 2026-11-03

**Base:** `main` (`998196a`), binario `~/.local/bin/litecord`  
**Entorno de referencia:** Linux x86_64, KDE Plasma Wayland, PipeWire 1.6 / WirePlumber, ALSA vía CPAL, salida F32 48 kHz estéreo.  
**Documentos relacionados:** `docs/UI_FLUIDITY_AND_MEMORY_ROADMAP.md`, `Auditar.md`, `docs/CODE_AUDIT_REPORT.md`.

Este documento ordena el trabajo pendiente en seis frentes: audio/voz, bandeja del sistema, llamadas DM, multimedia (imágenes/avatares), rendimiento y funciones sociales (soundboard/emojis). No es una lista de deseos: cada punto incluye el estado real del código, la causa probable, los archivos que hay que tocar y un criterio de aceptación medible.

---

## 0. Evidencia de la captura de audio (600 s)

Se capturó el monitor del sink por defecto entre **23:01:04 y 23:11:04** (600 s), mientras Litecord estaba conectado a un canal de voz.

| Métrica | Resultado |
|---|---|
| Duración | 600.0 s |
| Audio con energía tipo voz (`RMS > 0.008`) | **179.1 s** |
| Silencio (`RMS < 0.0008`) | **355.5 s** |
| Pico absoluto | 1.379 (monitor float, incluye otras fuentes del sistema) |
| Underrun de ALSA de Litecord | **1**, a las **23:08:42** (offset 457.185 s) |
| Paquetes RX de voz | **34 426** |
| DAVE OK | **27 804** |
| DAVE fallidos / descartados | **6 622 (19.2 %)** |
| Frames Opus decodificados | **27 804** |

El dato decisivo es que `decode_ok == dave_ok`: **no se decodificó ni un solo frame de PLC**. El código cree que está ocultando pérdidas, pero no lo está haciendo.

### 0.1. Causa raíz del “pop”

En `src/gateway/voice.rs` se llama a:

```rust
dec.decode(&[], 960, &mut pcm_out_buf[..])
```

en tres sitios para aplicar Packet Loss Concealment. Pero `opus-rs 0.1.26` hace esto al principio de `OpusDecoder::decode`:

```rust
if input.is_empty() {
    return Err("Input packet empty");
}
```

Es decir, **PLC nunca se ejecuta**. Cada paquete perdido, corrupto o con fallo DAVE se descarta y deja un hueco seco. El resultado coincide con lo que se escucha: un “pop” intermitente en la voz de los amigos, sobre todo cuando hay pérdida de paquetes o transiciones de época DAVE.

Además, cuando la cola de un SSRC se queda sin muestras, el callback de salida hace:

```rust
let decay_l = if hist[2].0.abs() < 0.0001 { 0.0 } else { hist[2].0 * 0.999 };
hist[3] = (decay_l, decay_r);
```

Eso no es un fade: en cuatro muestras (0,08 ms) la señal cae a cero. Al volver el siguiente paquete, `hist` se rellena de golpe con la nueva muestra. Es un escalón de amplitud, o sea, otro click audible.

El underrun de ALSA de las 23:08:42 ocurrió con la señal casi en silencio (RMS ≈ 0.0005), así que **no es la causa principal** de los pops en voz. El problema principal es la combinación de **19,2 % de paquetes descartados + PLC roto + fade de 4 muestras**.

### 0.2. Lo que sí queda descartado

- No se observaron picos de CPU ni errores de PipeWire/WirePlumber durante la captura.
- El underrun de ALSA existió, pero fue puntual y en silencio; hay que corregirlo igualmente, pero no explica los pops constantes.
- No se pudo aislar un “click” aislado en la forma de onda porque el artefacto ocurre dentro de voz y con pérdidas, no como un impulso limpio cada X segundos.

**Conclusión:** el pop es un problema de **concealment de voz**, no de hardware. La prioridad es arreglar el PLC y reducir los descartes DAVE; después, hacer el camino de salida tolerante a fallos.

---

## 1. P0 — Audio y voz

### 1.1. Arreglar el PLC de Opus (bloqueante)

**Objetivo:** que cada pérdida genere 20 ms de audio concebido por Opus en vez de un hueco.

**Archivos:** `src/gateway/voice.rs` (bloque de decodificación, ~líneas 575–1010).

**Cambios:**

1. Guardar el último TOC válido por SSRC:

```rust
let mut ssrc_last_toc: HashMap<u32, u8> = HashMap::new();
```

Al decodificar un paquete bueno:

```rust
ssrc_last_toc.insert(ssrc_recv, raw_opus[0]);
```

2. Donde hoy se llama `decode(&[], ...)`, llamar con un paquete de 1 byte (ToC del último frame bueno):

```rust
let toc = ssrc_last_toc.get(&ssrc_recv).copied().unwrap_or(0x78);
if let Ok(samples) = dec.decode(&[toc], 960, &mut pcm_out_buf[..]) {
    decode_success = true;
    decoded_count = samples;
    diag_rx.rx_plc_frames.fetch_add(1, Ordering::Relaxed);
}
```

`opus-rs` considera `input.len() <= 1` como frame perdido/DTX y usa el modo anterior para PLC; con `input.is_empty()` no lo hace. Hay que pasar un byte, no un slice vacío.

3. Si no hay TOC previo (primer paquete perdido antes de oír al usuario), rellenar con silencio y **no** contar como PLC.

4. Añadir al diagnóstico `plc_frames` y `plc_errors` para verificar que ahora sí se ejecuta.

**Criterio de aceptación:**

- En una llamada de 10 min, `plc_frames > 0` cuando `dave_fail > 0`.
- `decode_ok` ya no es igual a `dave_ok`.
- No hay huecos de 20 ms en el PCM de RX del test (`LITECORD_TEST_RX_PCM`).

**Test unitario:** construir un `OpusDecoder`, decodificar un frame válido y luego `decode(&[toc], ...)`; verificar que devuelve 960 muestras y que no devuelve `Err`.

---

### 1.2. Reducir los fallos DAVE (19,2 % es demasiado)

**Archivos:** `src/gateway/voice.rs` (~780–900), `src/gateway/types.rs`.

**Diagnóstico:** el 19,2 % de los paquetes RX se descartan antes de Opus. No todos son pérdida de red: muchos son paquetes **sin marcador DAVE** (`0xFAFA`) que el código rechaza cuando `plaintext_allowed()` es falso. También puede haber SSRC sin mapear a usuario y transiciones de época.

**Cambios:**

1. **Silencio/DTX no cifrado:** si el payload es `[0xF8, 0xFF, 0xFE]` (Opus silence) o un paquete DTX de 1–3 bytes, tratarlo como silencio válido, no como fallo DAVE. No hace falta PLC para un silencio explícito.
2. **Remitentes no DAVE:** si no hay `0xFAFA` y el SSRC está mapeado a un usuario que no está en el grupo DAVE (`s.get_user_ids()`), permitir passthrough de Opus. Los bots y clientes viejos no implementan DAVE; hoy se descartan.
3. **Mapeo SSRC → usuario:** no usar `ssrc as u64` como fallback durante más de una ventana corta. Mantener un `pending_ssrc` y reintentar el mapeo con OP5; mientras tanto, aplicar PLC en vez de descartar.
4. **Transiciones de época:** al recibir OP 22/29, no invalidar el decodificador de audio; `davey` ya maneja el ratchet. Verificar que `prepare_protocol`/`execute` no dejan el decryptor en un estado que rechace frames válidos.
5. **Logs por SSRC:** añadir contadores `ok/fail/no_magic/late` por SSRC y loguear cada 5 s. Sin esto no se puede saber si el 19 % es un bot, un amigo con cliente viejo o un bug de mapeo.

**Criterio de aceptación:**

- `dave_fail / RX < 1 %` en una llamada normal sin bots.
- Si hay un bot no DAVE, su audio se oye (passthrough) o se muestra como “silencio”, pero no genera 20 ms de PLC por paquete.
- Cero paquetes descartados por “sin marcador” cuando el emisor es un usuario conocido no DAVE.

---

### 1.3. Fade/crossfade correcto en la salida

**Archivo:** `src/gateway/voice.rs` (callbacks F32/I16, ~1200–1360).

Hoy, al quedarse sin cola, la señal cae a cero en 4 muestras y al volver el paquete salta. Sustituir por:

- **Starvation:** mantener la última muestra y aplicar un fade lineal o equal-power a cero en **5–10 ms** (240–480 muestras), no en 4.
- **Resume:** durante **5 ms**, crossfade entre el último valor retenido y el primer frame nuevo. Si el hueco fue mayor a 50 ms, hacer fade-in desde cero.
- **Gaps > 4 paquetes:** no saltar la secuencia sin más. Insertar hasta 5 frames de PLC y luego hacer fade/reset; nunca un salto seco.
- **Nuevo talkspurt:** si `is_new_talkspurt`, no resetear el histograma a cero de golpe; hacer un crossfade corto con el estado anterior.

**Criterio de aceptación:** un test con pérdida sintética del 10 % no debe producir muestras con derivada > 0,25 ni saltos de amplitud > 0,2 en 1 ms.

---

### 1.4. Sacar locks y allocations del callback de audio

**Archivo:** `src/gateway/voice.rs`.

El callback de salida hace, por cada callback:

- `sq_f32.lock()`
- `ssrc_to_userid_f32.lock()`
- `get_user_audio_settings()` (otro lock)
- `ssrc_vol_map` (HashMap nuevo)
- `aec_ref` (Vec nuevo)
- `queues.iter()` y `VecDeque::pop_front`

Eso es demasiado para un hilo de tiempo real. Si el callback llega tarde, ALSA hace underrun y aparece otro pop.

**Cambios:**

1. El callback solo debe **copiar** desde un ring buffer SPSC prellenado; la mezcla, el volumen y el PLC van en un hilo dedicado.
2. Preasignar `ssrc_vol_map`, `aec_ref`, `ready_ssrcs` y `to_remove` una vez por stream, no por callback.
3. Mover `get_user_audio_settings` a un snapshot atómico o a un `ArcSwap` actualizado por el hilo de control.
4. Medir el tiempo del callback y loguear si supera el 50 % del período.

**Criterio de aceptación:** 30 min de llamada con `pw-top` sin `ERR`, y `output_err_count == 0`.

---

### 1.5. Jitter buffer y prebuffer adaptativo

**Archivo:** `src/gateway/voice.rs` (`VOICE_PREBUFFER_SAMPLES = 1920`, `VOICE_RESET_INACTIVE_SAMPLES = 2880`).

Hoy el prebuffer es fijo (40–60 ms). Si la red tiene jitter > 60 ms, la cola se vacía y entra el fade de 4 muestras.

**Cambios:**

1. Medir el jitter por SSRC (diferencia entre `arrival_time` y `rtp_timestamp`).
2. Prebuffer objetivo: `clamp(40 ms + 2 × jitter_p95, 40 ms, 120 ms)`.
3. Reordenar paquetes con una ventana pequeña (2–3 paquetes) por número de secuencia RTP.
4. Descartar paquetes tardíos con fade, no con `drain` seco.
5. Tests con jitter sintético (0/30/80/150 ms) y pérdida 0/5/10 %.

---

### 1.6. Instrumentación permanente

Añadir a `VoiceDiag` (o a un archivo de log opcional):

- `plc_frames`, `plc_errors`
- `rx_dropped_no_magic`, `rx_dropped_dave`, `rx_dropped_late`
- `callback_max_us`, `callback_missed_deadline`
- `prebuffer_target_ms`, `prebuffer_actual_ms`
- `underruns` (contador de ALSA/CPAL)

Y un modo de diagnóstico accesible desde Ajustes → Voz: “Guardar 60 s de audio recibido” que use el mismo camino que `LITECORD_TEST_RX_PCM`, sin necesidad de compilar con `automation`. Eso permite analizar el próximo pop con audio limpio, sin música ni notificaciones del sistema.

---

## 2. P0 — Bandeja: clic izquierdo para mostrar/ocultar

### 2.1. Estado actual

- `src/utils/tray.rs`: usa `tray-icon 0.19`, menú con “Exibir Litecord” y “Sair”, y `with_menu_on_left_click(true)`.
- `src/main.rs` (~410–513): el handler de `TrayIconEvent` responde a `Click` y `DoubleClick` izquierdo enviando **siempre restaurar**.
- No existe un estado “ventana visible + enfocada” en el handler; `APP_IS_VISIBLE` ya existe y se actualiza al ocultar/restaurar.
- En Linux, `tray-icon` puede depender de AppIndicator y no entregar `activate`; el hint de `ksni` es correcto para KDE Plasma Wayland.

### 2.2. Comportamiento objetivo

- Clic izquierdo:
  - ventana visible y enfocada → ocultar a la bandeja;
  - ventana oculta, minimizada o sin foco → mostrar, restaurar y enfocar.
- Clic derecho: menú contextual (Abrir, Reproducir/Pausar, Detener, Salir).
- Un solo clic no debe abrir el menú.
- Doble clic no debe disparar dos toggles.

### 2.3. Implementación

1. Definir una acción explícita:

```rust
enum TrayAction {
    Toggle,
    Restore,
    Quit,
}
```

2. En `SystemTrayManager::setup`, usar `with_menu_on_left_click(false)` y dejar el menú solo para botón derecho.

3. En `main.rs`, enviar `TrayAction::Toggle` en `TrayIconEvent::Click { button: Left, .. }`. Ignorar `DoubleClick` o aplicar debounce de 250 ms.

4. Extraer la lógica de ocultar de `hide_to_tray` a una función reutilizable:

```rust
fn hide_main_to_tray(app: &slint::Weak<AppWindow>, hwnd: &Arc<Mutex<Option<isize>>>) { ... }
```

El handler de toggle la llama si `APP_IS_VISIBLE.load(Relaxed)` y la ventana tiene foco:

```rust
let focused = ui.window().with_winit_window(|w| w.has_focus());
if APP_IS_VISIBLE.load(Relaxed) && focused {
    hide_main_to_tray(...);
} else {
    restore_main_from_tray(...);
}
```

5. En Linux, si `tray-icon` no entrega `Click` en KDE, añadir un backend `ksni` solo para Linux:

```rust
impl ksni::Tray for TrayBackend {
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayAction::Toggle);
    }
}
```

Mantener `tray-icon` en Windows/macOS y usar `ksni` en Linux. El menú de `ksni` puede replicar Abrir/ Salir.

6. Wayland: si `focus_window()` es ignorado, usar `xdg_activation_v1` (token de activación) o, como último recurso, recrear la superficie al restaurar. No hacer `set_visible(true)` sin token y asumir que KWin dará foco.

### 2.4. Criterios de aceptación

- KDE Wayland: clic izquierdo alterna ocultar/restaurar; clic derecho abre menú.
- Windows: mismo comportamiento con `WM_LBUTTONUP`.
- Sin doble toggle al hacer doble clic.
- La ventana restaurada queda enfocada o, si el compositor lo impide, se documenta el fallback.

---

## 3. P1 — Llamadas DM: tono, estado y UI

### 3.1. Estado actual

- `ui/appwindow.slint` ya tiene `is_in_dm_call`, `has_incoming_call`, `incoming_call_*` y un panel de llamada entrante con Aceptar/Rechazar.
- `src/main.rs` `on_call_dm` pone `is_in_dm_call = true` inmediatamente, abre micrófono, envía OP4 y dispara `ring_dm_call`; no hay estado “ringing”, ni tono de llamada, ni detección de respuesta.
- `src/http.rs` ya tiene `ring_dm_call` y `stop_ringing_dm_call`.
- `src/audio/sound_effects.rs` solo genera Mute/Unmute/Deafen/Undeafen/Join/Leave; no hay ringback/ringtone.
- No hay estado de reconexión de voz visible.

### 3.2. Máquina de estados

Añadir en Rust:

```rust
enum CallState {
    Idle,
    OutgoingRinging { channel_id: String, started: Instant },
    IncomingRinging { channel_id: String, caller: String },
    Connecting { channel_id: String },
    Active { channel_id: String, started: Instant },
    Reconnecting { channel_id: String },
    Ended { channel_id: String, reason: String },
}
```

`on_call_dm` pasa a `OutgoingRinging`, no a `Active`. El estado `Active` se activa cuando:

- llega `VOICE_STATE_UPDATE` de otro usuario al canal, o
- el voice gateway confirma `VOICE_SERVER_UPDATE`/sesión y hay al menos otro participante.

### 3.3. Tonos

Añadir a `sound_effects` un **loop** de tonos, no un stream CPAL nuevo por tono:

- `CallRingback`: 440 + 480 Hz, 2 s on / 4 s off (EE. UU.) o patrón configurable.
- `CallRingtone`: patrón más brillante para llamada entrante.
- `CallConnect`: dos notas ascendentes.
- `CallDisconnect`: dos notas descendentes.
- `CallReconnect`: pulso suave cada 2 s.

El loop debe vivir en el mismo hilo `ui-sound-player` o en un mixer dedicado, con `start/stop` por `CallState`. No debe solaparse con `JoinChannel`/`LeaveChannel`; si hay llamada, silenciar el chime de canal.

### 3.4. UI

- En DM, al llamar: mostrar en el header/chat una tarjeta “Llamando a @X…” con avatar(es) **parpadeando** (animación de opacidad o anillo) y botón Cancelar.
- Cuando el otro acepta: la tarjeta pasa a “En llamada 00:12” con mute/deafen/colgar.
- Llamada entrante: panel existente + ringtone; al aceptar, detener ringtone y reproducir connect.
- Si el usuario está en un canal de voz de servidor y llama a un DM: primero salir del canal con `LeaveChannel`, guardar `previous_voice_channel`, y luego iniciar la llamada. Al colgar, ofrecer “Volver a <canal>” y reconectar con tono.
- Reconexión: si el voice WS/UDP se cae, mostrar “Reconectando…” y reproducir `CallReconnect`; al volver, `CallConnect`. Si falla, `CallDisconnect` y “Llamada finalizada”.

### 3.5. Criterios de aceptación

- Al llamar se oye ringback local y el otro usuario recibe ring.
- La UI no dice “en llamada” hasta que el otro acepta.
- Al aceptar/rechazar/colgar se detiene el tono correcto.
- Si se cae la voz, hay estado visible y tono; al reconectar se restaura el audio.
- Test unitario de la máquina de estados (timeout 60 s, rechazo, busy, reconexión).

---

## 4. P1 — Imágenes en mensajes: visor ampliado

### 4.1. Estado actual

- `ui/components/chat_messages_view.slint` (~1085) muestra adjuntos de imagen a 320 px de ancho, con botón de descarga y preview.
- `src/utils/attachment_cache.rs` ya tiene `full_cache`, `preview_cache` y descarga asíncrona.
- No hay callback para abrir una imagen en grande.

### 4.2. Implementación

1. Añadir `callback open_image_viewer(string att_id, string url);` en `appwindow.slint`.
2. En `chat_messages_view.slint`, hacer clic en la imagen (cuando `is_downloaded`) o en el botón de descarga (cuando no) para abrir el visor.
3. Crear `ImageViewer` en Slint:
   - fondo `surface-scrim` con blur suave;
   - imagen `image-fit: contain`, centrada, máximo 90 % de la ventana;
   - zoom con rueda (1×–8×), arrastre para pan, doble clic para 1×/2×;
   - `Esc` o clic fuera para cerrar;
   - botones: descargar, copiar, abrir externo, anterior/siguiente si hay varias.
4. Si la imagen no está descargada, el visor muestra el preview y dispara `download_full_async`; al terminar, actualiza el visor.
5. Reutilizar `full_img` del cache; no decodificar en el hilo de UI.
6. Soportar también avatares, embeds e imágenes de servidor.

### 4.3. Criterios de aceptación

- Clic en imagen abre visor sin reflow del chat.
- Zoom/pan fluidos a 60 FPS con imagen 4K.
- `Esc` cierra y devuelve el foco al chat.
- No se descarga dos veces la misma imagen.

---

## 5. P1 — Avatares que no cargan

### 5.1. Causas probables

- `get_user_avatar_url` devuelve vacío cuando el usuario no tiene avatar global; la UI no muestra el avatar por defecto.
- Los avatares animados (`a_...`) se piden como `.webp?size=64`; algunos CDN/clientes esperan `.gif` o `animated=true`.
- Avatares de miembro de servidor (`guild_avatar`) no se distinguen del avatar global.
- Un fallo transitorio de CDN/proxy se cachea como fallo 30 s (`RETRY_AFTER`), y la UI queda en placeholder.
- En DMs, no todos los `recipients` se registran antes de construir la lista.

### 5.2. Implementación

1. **Avatar por defecto:** si no hay hash, generar `https://cdn.discordapp.com/embed/avatars/{index}.png` (índice por `user_id` o discriminador) y mostrarlo, no un hueco.
2. **Animados:** si el hash empieza por `a_`, usar `.gif?size=128` o `.webp?size=128&animated=true`; cachear el primer frame si el formato no es soportado.
3. **Guild avatar:** clave `(guild_id, user_id)` y URL `/guilds/{guild_id}/users/{user_id}/avatars/{hash}.png`; usarlo en listas de miembros y voz.
4. **DMs:** registrar `recipients` y `user` de cada DM antes de `slot_for`; si falta, hacer fallback a `/users/{id}` y refrescar.
5. **Reintentos:** backoff exponencial (1 s, 2 s, 5 s, 15 s) en vez de un bloqueo fijo de 30 s; no borrar el último avatar bueno.
6. **Diagnóstico:** log opcional `avatar: user=... url=... status=...` y contador de fallos por host.
7. **UI:** placeholder con iniciales + color estable; shimmer solo si la carga está en curso.

### 5.3. Criterios de aceptación

- 100 % de usuarios con avatar global se ven en sidebar, chat, voz y perfil.
- Usuarios sin avatar muestran el avatar por defecto, no un círculo vacío.
- Un fallo de red no deja el avatar bloqueado más de 5 s.
- Test unitario para URL global/guild/animado/default.

---

## 6. P1/P2 — RAM y fluidez

Ya existe `docs/UI_FLUIDITY_AND_MEMORY_ROADMAP.md` con F1–F19 cerrados. Este roadmap **no repite** ese trabajo; añade lo que quedó como deuda y lo que impacta directamente en la sensación de fluidez.

### 6.1. RAM al inicio

**Hipótesis:** el pico inicial viene de:

- `READY` + `READY_SUPPLEMENTAL` de 22 guilds (canales, miembros, presencias, emojis, voice states).
- Decodificación de iconos de guild y avatares en ráfaga.
- Construcción de modelos de sidebar/DMs y cache de mensajes.
- Framebuffer software de 2880×1620 (~35 MiB) + buffers de video/imagen.

**Tareas:**

1. Añadir instrumentación de arranque: `Instant` y RSS en hitos (`READY`, `READY_SUPPLEMENTAL`, primer frame, 8 s, 30 s). Loguear una tabla única.
2. Carga perezosa de guilds: procesar solo el guild activo + los visibles; el resto se hidrata al hacer clic. No descartar `guilds_map` (ya se decidió que no compensa), pero sí evitar decodificar iconos/emojis/miembros de todos a la vez.
3. Limitar la decodificación de iconos a un presupuesto por frame (p. ej. 4 iconos cada 16 ms) y priorizar visibles.
4. No pre-cargar avatares de todos los miembros; solo los que se van a pintar.
5. Medir con `/proc/<pid>/smaps_rollup` y `heaptrack` si disponible; objetivo: **RSS < 180 MiB** tras 30 s (sin streaming) y **< 250 MiB** con chat con imágenes.
6. Si el pico es por `READY`, considerar procesar el payload en streaming (no clonar el `Value` completo) y liberar el JSON antes de construir modelos.

### 6.2. Fluidez al cambiar chat/servidor

**Tareas:**

1. Perfilar con `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console` y `perf record` durante 20 cambios de canal y 100 mensajes.
2. Cachear un **snapshot de vista** por canal: filas ya agrupadas, modelos de sidebar y scroll. Al volver a un canal, aplicar el snapshot y luego parchear solo lo nuevo.
3. Evitar reconstruir todo el modelo de chat al llegar un mensaje: usar `set_row_data`/append incremental (F2 ya lo hace; verificar que no se rompa al cambiar de canal).
4. Precargar el canal vecino (anterior/siguiente) en segundo plano, no el servidor entero.
5. Coalescer `invoke_from_event_loop` de avatares/emojis en un solo batch cada 60–120 ms.
6. A/B del renderer: `winit-software` vs GPU (`SLINT_BACKEND=winit-software` / `winit` con OpenGL). Medir frame time y RSS; elegir por hardware, no por fe.
7. Reducir `drop-shadow-blur` y `clip` anidados en listas largas; evitar animaciones de opacidad en filas que se reciclan.

### 6.3. Criterios de aceptación

- Cambio de canal: **p95 < 80 ms** hasta contenido visible.
- Cambio de servidor: **p95 < 120 ms**.
- Scroll de chat: 60 FPS estables en 1080p con 150 filas.
- RSS estable después de 20 cambios (sin crecimiento > 10 %).

---

## 7. P2 — Soundboard completo (servidor + Nitro)

### 7.1. Estado actual

- `src/audio/soundboard.rs` sintetiza 8 sonidos (airhorn, bruh, quack, badumtss, vineboom, tada, applause, cricket) y los inyecta en la cola del micrófono.
- `ui/appwindow.slint` tiene un panel de 2×4 en el input del chat y un botón 🔊, no en la cabecera de voz.
- No hay sonidos de servidor, ni descarga de CDN, ni eventos de gateway, ni panel junto a la cámara.

### 7.2. API de Discord (verificada)

- `GET /soundboard-default-sounds` → sonidos por defecto.
- `GET /guilds/{guild.id}/soundboard-sounds` → `{ items: [...] }`.
- Gateway **Op 31** (`Request Soundboard Sounds`) con `guild_ids`.
- Eventos: `GUILD_SOUNDBOARD_SOUND_CREATE`, `GUILD_SOUNDBOARD_SOUND_UPDATE`, `GUILD_SOUNDBOARD_SOUND_DELETE`, `GUILD_SOUNDBOARD_SOUNDS_UPDATE`.
- Sonido: `sound_id`, `name`, `volume`, `emoji_id`, `emoji_name`, `guild_id`, `available`.
- Archivo: `https://cdn.discordapp.com/soundboard-sounds/{sound_id}` (MP3/Ogg).

### 7.3. Implementación

1. **Modelo** en `src/audio/soundboard.rs` o nuevo `src/audio/soundboard_catalog.rs`:

```rust
pub struct SoundboardSound {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub volume: f32,
    pub guild_id: Option<String>,
    pub available: bool,
    pub source: SoundSource, // Default | Guild | OtherGuild
}
```

2. **Catálogo:**
   - al conectar, pedir defaults y sonidos del guild actual;
   - Op 31 para el resto de guilds;
   - guardar en `OnceLock<Mutex<HashMap<...>>>` con límite y LRU;
   - actualizar con los eventos de gateway.
3. **Descarga y decode:**
   - cachear en `~/.cache/Litecord/soundboard/{sound_id}.ogg` (o `.mp3`);
   - decodificar con `symphonia` (Ogg/Vorbis/MP3) o `lewton` + `minimp3` en `spawn_blocking`;
   - remuestrear a 48 kHz estéreo y normalizar por `volume`;
   - no bloquear el hilo de UI ni el de voz.
4. **Reproducción:**
   - reutilizar `play_soundboard_effect` generalizado a `play_soundboard_pcm(samples, volume)`;
   - inyectar en la cola del micrófono en frames de 960 muestras (20 ms), con fade-in/out de 5 ms;
   - reproducir localmente por el mismo mixer de UI, sin abrir un stream CPAL por sonido;
   - opcional: `POST /channels/{id}/send-soundboard-sound` para el evento visual, pero el audio debe ir por la cola de voz (E2EE).
5. **UI:**
   - mover/duplicar el botón de soundboard a la **cabecera de voz, a la izquierda de Cámara** (`ui/appwindow.slint` ~1636);
   - panel con pestañas: “Este servidor”, “Discord”, “Otros servidores”, “Recientes/Favoritos”;
   - búsqueda, preview local (solo yo), volumen, atajo `Ctrl+\``;
   - mostrar emoji y nombre; indicador de “Nitro-Free” solo si se quiere, sin bloquear.
6. **Nitro:** no filtrar por `available` ni por premium; mostrar todos los sonidos descargables. Esto puede violar los Términos de Discord; documentarlo como riesgo y no empaquetar assets de Nitro.
7. **Permisos:** respetar `USE_SOUNDBOARD`/`USE_EXTERNAL_SOUNDS` para enviar; si no hay permiso, permitir preview local y mostrar aviso.

### 7.4. Criterios de aceptación

- Panel junto a Cámara en la cabecera de voz.
- Sonidos del servidor actual aparecen y suenan.
- Sonidos por defecto de Discord aparecen y suenan.
- Sonidos de otros servidores se pueden reproducir localmente y, si el permiso lo permite, enviar.
- Sin bloqueo de UI durante descarga/decode.
- Test de parsing de API y de inyección de PCM.

---

## 8. P2 — Emojis completos (Unicode + servidores + Nitro)

### 8.1. Estado actual

- `ui/appwindow.slint` tiene solo **36 emojis Unicode** hardcodeados en `emoji_rows`.
- `src/utils/emoji_cache.rs` ya registra emojis personalizados (`register_custom_emoji`, `get_all_custom_emojis`) desde READY/eventos, pero **el picker no los usa**.
- `src/ui/helpers.rs` renderiza emojis personalizados en mensajes; el picker no.
- No hay búsqueda ni categorías.

### 8.2. Implementación

1. **Catálogo Unicode completo:**
   - usar un JSON de Twemoji/Unicode (o el crate `emojis`) con nombre, categoría, keywords y código;
   - construir filas bajo demanda en Rust y asignarlas a `emoji_rows`;
   - renderizar con Twemoji (`u_` en `emoji_cache`) para color consistente.
2. **Emojis de servidor:**
   - mantener `HashMap<guild_id, Vec<CustomEmojiEntry>>`;
   - llenar desde `READY`, `GUILD_EMOJIS_UPDATE`, `GUILD_CREATE` y REST de respaldo;
   - incluir `animated`; descargar `.gif` o `.webp` animado; cachear;
   - el picker muestra todos los servidores, no solo el activo.
3. **Sin bloqueo Nitro:** no filtrar por `premium`/`available`; insertar `<:name:id>` o `<a:name:id>` en el input. Si el servidor rechaza el envío, mostrar el error y dejar el texto.
4. **UI:**
   - pestañas “Unicode” / “Servidores” / “Recientes”;
   - búsqueda por nombre y keyword;
   - grid virtualizado o por filas; no construir miles de imágenes en el hilo de UI;
   - reacciones y mensajes usan el mismo cache.
5. **Riesgo:** usar emojis externos sin Nitro puede violar ToS; documentarlo.

### 8.3. Criterios de aceptación

- Picker con todos los Unicode y todos los emojis de los servidores.
- Búsqueda instantánea (< 50 ms) y sin congelar.
- Emojis animados se ven animados (o primer frame si el backend no soporta GIF).
- Enviar un emoji de otro servidor no queda bloqueado por la UI.

---

## 9. Fases, dependencias y estimación

| Fase | Contenido | Días | Depende de |
|---|---|---:|---|
| **F0** | Instrumentación de audio + diagnóstico por SSRC + captura limpia | 1–2 | — |
| **F1** | PLC real, DAVE, fade/crossfade, callback sin locks, jitter buffer | 4–6 | F0 |
| **F2** | Bandeja toggle (Linux `ksni` + Windows) | 1–2 | — |
| **F3** | Llamadas DM (estado, tonos, UI, reconexión) | 4–6 | F1 |
| **F4** | Visor de imágenes + avatares (default/guild/animado/retry) | 3–5 | — |
| **F5** | RAM/fluidez: perfilado, snapshots de canal, carga perezosa | 4–6 | F0 |
| **F6** | Soundboard: catálogo, CDN, decode, UI junto a cámara | 5–7 | F1 |
| **F7** | Emojis: catálogo Unicode + servidores + picker | 3–5 | F6 (comparte caché/descarga) |

**Total orientativo:** 4–6 semanas de trabajo efectivo, sin contar revisión manual de audio en llamadas reales.

### Orden recomendado

1. **F0/F1** primero: sin PLC real, cualquier otra mejora de audio es cosmética.
2. **F2** en paralelo (es independiente y de bajo riesgo).
3. **F5** después de F1: el perfilado de arranque se beneficia de la instrumentación.
4. **F3/F4** en paralelo.
5. **F6/F7** al final, porque dependen de un pipeline de descarga/decode estable.

---

## 10. Verificación

### 10.1. Tests automáticos

```bash
cargo test --offline --bin litecord
cargo test --offline --bin litecord gateway::voice -- --nocapture
cargo test --offline --bin litecord soundboard -- --nocapture
cargo test --offline --bin litecord emoji_cache -- --nocapture
```

Añadir tests nuevos:

- PLC con 1 byte de TOC devuelve 960 muestras.
- Pérdida sintética 0/5/10/20 % no produce saltos > 0,2.
- `TrayAction` toggle: visible+foco → hide; oculto → restore; doble clic = un toggle.
- `CallState` timeout/rechazo/reconexión.
- URL de avatar global/guild/animado/default.
- Parseo de `soundboard-default-sounds` y `guild soundboard-sounds`.
- Merge de emojis Unicode + servidores.

### 10.2. Pruebas manuales

- Llamada real de 30 min con 3–5 personas; revisar `VoiceDiag` y `pw-top`.
- Captura limpia con `LITECORD_TEST_RX_PCM` (build automation) y detector de clicks.
- Alternar bandeja 20 veces en KDE Wayland y Windows.
- Llamada DM: aceptar, rechazar, colgar, reconectar, cambiar de canal.
- Abrir 20 imágenes 4K y hacer zoom/pan.
- Navegar 20 canales/servidores y medir frame time/RSS.
- Reproducir 10 sonidos de servidor y 10 de otros servidores.
- Abrir el picker con 5 000 emojis y buscar.

### 10.3. Criterios globales

- 0 underruns de ALSA en 30 min de llamada.
- `dave_fail/RX < 1 %` sin bots; con bots, passthrough o silencio explícito.
- `plc_frames > 0` y `plc_errors == 0`.
- RSS < 180 MiB tras 30 s; sin crecimiento > 10 % tras 20 cambios.
- Cambio de canal p95 < 80 ms.
- 0 panics en 1 h de uso.

---

## 11. Riesgos y decisiones

1. **Nitro bypass:** mostrar/reproducir sonidos y emojis premium sin Nitro puede violar los Términos de Discord. Se puede implementar como “cliente no oficial”, pero conviene advertirlo y no empaquetar assets propietarios.
2. **PLC con `opus-rs`:** pasar un ToC de 1 byte es un workaround; si el port tiene bugs en el modo perdido, la alternativa es usar `libopus` por FFI o un decoder con PLC explícito. Medir antes/después.
3. **Callback de audio:** mover la mezcla fuera del callback es un cambio delicado; hacerlo detrás de un feature flag y A/B con `LITECORD_TEST_*`.
4. **Wayland:** el foco al restaurar puede ser rechazado por el compositor. El fallback de recrear superficie es la última opción; documentar que en algunos compositores el clic solo muestra, no enfoca.
5. **RAM:** no perseguir kilobytes en `guilds_map`; el pico está en el arranque y en la decodificación de imágenes. Medir primero.
6. **Soundboard y E2EE:** el audio debe ir por la cola de voz (Opus), no por un canal paralelo; el endpoint REST es solo para el evento visual. No romper DAVE.

---

## 12. Primer parche recomendado (si se quiere atacar ya)

Si solo hay tiempo para un cambio, este es el de mayor impacto:

1. `src/gateway/voice.rs`: guardar `ssrc_last_toc` y cambiar los tres `decode(&[], ...)` por `decode(&[toc], ...)`.
2. Añadir `rx_plc_frames` al diagnóstico.
3. Reemplazar el fade de 4 muestras por un fade de 5 ms y un crossfade de 5 ms al reanudar.

Con eso, el 19,2 % de paquetes que hoy se pierden en silencio pasarán a estar cubiertos por PLC, y el “pop” debería reducirse de forma drástica incluso antes de tocar DAVE o el jitter buffer.
