# Auditoría Técnica y Plan Definitivo de Correcciones — Litecord

**Fecha:** 21 de Octubre de 2026  
**Objetivo:** Auditoría exhaustiva de los 5 fallos críticos reportados en Litecord, verificación de especificaciones de protocolos (Discord Voice Gateway, DAVE E2EE, Slint UI layout engine, Wayland/KWin y Win32/macOS) y diseño de la solución técnica definitiva.

---

## 1. Doble icono de Litecord en la barra de tareas de KDE Plasma

### Hallazgo de Auditoría
- **Causa primaria comprobada en código ([`src/main.rs#L185`](file:///home/kes/Litecord/src/main.rs#L185)):**
  ```rust
  let popout_window = PopoutStreamWindow::new()?;
  ```
  La ventana `PopoutStreamWindow` se instancia al arrancar el programa en el hilo principal antes de cualquier interacción del usuario. En Slint con backend `winit`, invocar `::new()` crea inmediatamente un `xdg_toplevel` nativo registrado en KWin (Wayland).
- **Mecanismo de Wayland y `groupingStrategy`:**
  En [`~/.config/plasma-org.kde.plasma.desktop-appletsrc`](file:///home/kes/.config/plasma-org.kde.plasma.desktop-appletsrc#L194), el usuario tiene `groupingStrategy=0` (*Do Not Group*). Al crearse dos superficies de nivel superior con la misma clase de aplicación (`litecord`), KWin y el Task Manager de Plasma asignan un botón individual a cada ventana.
  Aunque en la [línea 329](file:///home/kes/Litecord/src/main.rs#L329) se llama a `winit_pop.set_visible(false)`, el protocolo `xdg_shell` en Wayland ya notificó al compositor de la existencia de la superficie toplevel independiente.
- **Validación Multiplataforma:**
  - En **Windows (Win32)**: Crear una ventana invisible en el arranque sin `WS_EX_TOOLWINDOW` añade un identificador en el registro de ventanas de la Shell si no tiene ventana padre.
  - En **macOS (AppKit)**: Un `NSWindow` instanciado sin ventana padre puede ser considerado ventana secundaria del Dock.

### Plan Definitivo de Corrección
1. **Instanciación Lazy (Perezosa):** Eliminar `PopoutStreamWindow::new()` del flujo de arranque de `main()`. Almacenar `Option<PopoutStreamWindow>` o instanciarla dinámicamente mediante `PopoutStreamWindow::new()` únicamente cuando el usuario haga clic en el botón de desacoplar transmisión (`on_popout_stream_window`).
2. **Jerarquía y Skip Taskbar:** Cuando se cree, asignarla como ventana secundaria (`transient_for` o `set_parent_window`) de la ventana principal de Litecord, previniendo que el gestor de ventanas del sistema operativo la trate como una aplicación separada.

---

## 2. Mensajes rotos e ilegibles en canales de "Juegos Gratis"

### Hallazgo de Auditoría
- **Causa A — Canales de tipo Foro (`ChannelType::GuildForum = 15`):**
  - Muchos servidores usan foros donde cada juego gratis es un hilo (`thread`).
  - La API de Discord rechaza `/channels/{forum_id}/messages` con **HTTP 400 Bad Request**.
  - En [`src/http.rs#L166-L189`](file:///home/kes/Litecord/src/http.rs#L166-L189), el fallback actual solo consulta los hilos activos (`get_forum_threads_active`) y genera mensajes simulados sin contenido real:
    ```rust
    "content": format!("📌 **{}**\n💬 {} respostas nesta oferta.", tname, total_msgs)
    ```
    No se descarga el mensaje inicial del hilo (`/channels/{thread_id}/messages?limit=1`), donde los bots colocan los enlaces a Steam/Epic, la clave y la fecha límite.
- **Causa B — Bug de Layout de Slint en mensajes con enlaces embebidos:**
  - En [`ui/components/chat_messages_view.slint#L471`](file:///home/kes/Litecord/ui/components/chat_messages_view.slint#L471) y [Línea 701](file:///home/kes/Litecord/ui/components/chat_messages_view.slint#L701):
    ```slint
    if line.blocks.length > 1: HorizontalLayout {
        alignment: start;
        spacing: 4px;
        for block in line.blocks: HorizontalLayout { ... }
    }
    ```
  - **Comportamiento interno de Slint:** `HorizontalLayout` **NUNCA** realiza salto de línea (wrap) bajo ninguna circunstancia. Si una línea contiene texto normal + enlace + emoji, Slint coloca todos los bloques en una sola línea horizontal que se desborda hacia la derecha fuera de la pantalla.
- **Causa C — Previews de Banners de Embeds sin descargar:**
  - Las imágenes del juego (`embed.image.url`) se asignan a `attachments` con `is_downloaded: false`, mostrándose como una caja gris vacía con un botón de descarga en vez del banner visual de la oferta.

### Plan Definitivo de Corrección
1. **Soporte real para Foros:** En [`src/http.rs`](file:///home/kes/Litecord/src/http.rs), para canales de foro, obtener los posts activos y cargar el mensaje inicial (`first_message`) de cada hilo para extraer el título, imagen, descripción y enlace de la tienda.
2. **Corrección del wrapping en Slint:** En [`chat_messages_view.slint`](file:///home/kes/Litecord/ui/components/chat_messages_view.slint), en lugar de forzar múltiples bloques en un `HorizontalLayout` rígido, representar el texto como texto continuo con word-wrap, separando los enlaces destacados en bloques interactivos individuales o mediante formato compatible.
3. **Carga visual automática de Banners:** Cargar y mostrar directamente la imagen del embed (`embed.image.url`) en la tarjeta del mensaje sin requerir interacción manual.

---

## 3. "No veo a mi amigo streameando"

### Hallazgo de Auditoría (Especificación Discord Voice & DAVE)
- **Causa A — Incompatibilidad de Transporte (P2P Cloudflare vs Discord Voice SFU):**
  - El código en [`src/screen_capture/signaling.rs`](file:///home/kes/Litecord/src/screen_capture/signaling.rs) conecta a un WebSocket de Cloudflare Workers propio (`litecord-signal...`) para P2P. Un amigo que transmita desde Discord oficial **nunca** se conecta a ese servidor.
- **Causa B — Opcode de Discord Voice Gateway para Ver Streams:**
  - *Corrección de auditoría sobre el diagnóstico previo:* En la Discord Voice Gateway (v4/v5/v8), el comando para ver la transmisión de otro usuario es **Opcode 20 (`Stream Watch`)** (no el 12):
    ```json
    {
      "op": 20,
      "d": {
        "stream_key": "guild:{guild_id}:{channel_id}:{user_id}"
      }
    }
    ```
  - Al recibir el Opcode 20, el Voice Gateway asigna un **SSRC de Video** para ese usuario.
  - Como Litecord ya tiene implementado el protocolo **DAVE (MLS End-to-End Encryption)** para audio (confirmado en logs: `DAVE: Welcome processado com SUCESSO!`), las mismas claves simétricas ratcheted por participante en DAVE se usan para desencriptar los paquetes RTP de video H.264 entrantes.
  - Al no enviar el Opcode 20, los servidores de Discord no rutean los paquetes RTP de video a la IP/puerto UDP de Litecord.
- **Causa C — Falla de reactividad en la UI de Slint:**
  - En [`src/main.rs#L4847`](file:///home/kes/Litecord/src/main.rs#L4847), `GatewayEvent::VoiceStatesUpdated` solo llama a `update_channel_voice_rows` (sidebar), pero **no** reconstruye ni notifica a `voice_participants`.
  - En [`ui/appwindow.slint#L1643`](file:///home/kes/Litecord/ui/appwindow.slint#L1643), la propiedad `has_active_stream` se queda en `false`, manteniendo la tarjeta en altura de 76px (solo voz) y ocultando el visor de video.

### Plan Definitivo de Corrección
1. **Envío de Opcode 20 (`Stream Watch`):** En [`src/gateway/voice.rs`](file:///home/kes/Litecord/src/gateway/voice.rs), cuando un peer en el canal de voz emite `self_stream: true` (o al hacer clic en "Ver transmisión"), enviar Opcode 20 con su `stream_key`.
2. **Recepción RTP y Pipe a MPV Zero-Copy:** Identificar el SSRC del video entrante, desencriptar mediante DAVE/AEAD y canalizar el stream H.264 por stdin al proceso `MpvStreamPlayer` ya existente.
3. **Refresco inmediato en Slint:** Actualizar de inmediato el modelo `voice_participants` con `p.is_streaming = true` y activar `root.has_active_stream = true`.

---

## 4. Emojis incompletos e interfaz rota

### Hallazgo de Auditoría
- **Causa A — Bug de Elementos Fantasma en Slint:**
  - En [`ui/appwindow.slint#L3044-L3114`](file:///home/kes/Litecord/ui/appwindow.slint#L3044-L3114):
    ```slint
    HorizontalLayout {
        spacing: 4px;
        for e[idx] in root.emoji_list: Rectangle {
            if idx < 12 : Rectangle {
                width: 34px; height: 34px; ...
            }
        }
    }
    ```
  - **Mecanismo del compilador de Slint:** `for` crea 36 rectángulos en el layout. El `if` interior condicionaliza los hijos, pero el `Rectangle` exterior sigue existiendo en el árbol de layout.
  - Para 36 emojis en una fila con `idx < 12`, se generan **12 rectángulos visibles + 24 rectángulos vacíos** con `spacing: 4px` (96px de espacio invisible fantasma).
  - En la fila 2 (`idx >= 12 && idx < 24`), los 12 rectángulos vacíos iniciales empujan a los visibles fuera del borde derecho.
  - En la fila 3 (`idx >= 24`), 24 rectángulos vacíos desplazan los emojis completamente fuera de la vista.
- **Causa B — Lista incompleta y sin emojis del servidor:**
  - Solo hay 36 emojis Unicode fijos. No se sincronizan los emojis personalizados del servidor (`guild_emojis`) almacenados en [`src/utils/emoji_cache.rs`](file:///home/kes/Litecord/src/utils/emoji_cache.rs).
- **Causa C — Layout vertical:**
  - El selector fue incrustado directamente en el `VerticalLayout` del chat, desplazando 240px la conversación cada vez que se abre.

### Plan Definitivo de Corrección
1. **Eliminar los Elementos Fantasma con un Modelo Bidimensional:**
   - Crear una estructura por fila:
     ```slint
     export struct EmojiRow { emojis: [EmojiPickerItem] }
     in-out property <[EmojiRow]> emoji_rows;
     ```
   - En Slint:
     ```slint
     for row in root.emoji_rows: HorizontalLayout {
         spacing: 4px;
         alignment: start;
         for e in row.emojis: Rectangle {
             width: 34px; height: 34px;
             // render limpio sin condicionales de índice
         }
     }
     ```
     Esto garantiza 0 elementos fantasma y alineación milimétrica.
2. **Integrar Emojis de Servidor y Bypass FakeNitro:**
   - Población dinámica de `emoji_rows` con emojis de Unicode estándar y emojis personalizados del servidor activo.
   - Envío automático con CDN URL (`https://cdn.discordapp.com/emojis/{id}.webp`) para servidores sin Nitro.
3. **Popover Flotante:** Posicionar el contenedor como un overlay absoluto sobre el input de chat.

---

## 5. Panel de sonido con interfaz rota

### Hallazgo de Auditoría
- **Causa A — Elementos Fantasma en el Grid de Sonidos:**
  - En [`ui/appwindow.slint#L3192-L3260`](file:///home/kes/Litecord/ui/appwindow.slint#L3192-L3260):
    ```slint
    HorizontalLayout {
        spacing: 8px;
        for s[idx] in root.soundboard_list: Rectangle {
            if idx < 4 : Rectangle {
                horizontal-stretch: 1;
                ...
            }
        }
    }
    ```
  - Se repite exactamente el mismo fallo: para 8 sonidos, la primera fila genera 4 rectángulos expandidos (`stretch: 1`) y 4 rectángulos vacíos (`stretch: 0`), causando un colapso en el cálculo de anchos del motor de Slint.
- **Causa B — Inyección destructiva en el chat:**
  - Empuja 220px estáticos en el historial de mensajes de la sala.

### Plan Definitivo de Corrección
1. **Modelo de 2 Filas Limpio:**
   - Dividir `soundboard_list` en dos colecciones exactas: `soundboard_row1: [SoundboardItem]` y `soundboard_row2: [SoundboardItem]` (o `[SoundboardRow]`).
   - Cada `HorizontalLayout` itera únicamente sobre sus 4 elementos reales con `horizontal-stretch: 1`, logrando una cuadrícula perfectamente simétrica.
2. **Superposición (Overlay):** Mostrar el panel anclado en la parte inferior sobre el input de chat sin alterar el scroll del historial de mensajes.
