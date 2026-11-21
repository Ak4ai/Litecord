# Litecord — Cierre e nuevo roadmap (2026-11-03)

Este documento reemplaza como **estado vivo** al roadmap
`ROADMAP_MEJORAS_2026-11-03.md`. Aquel sigue siendo útil como análisis técnico
del pop de audio, pero aquí se registra qué se implementó, qué se verificó y qué
queda para las siguientes iteraciones.

## 1. Implementado en esta iteración

### 1.1. Audio / voz
- **PLC real de Opus.** Los tres puntos que llamaban `decode(&[], ...)` ahora
  pasan el último TOC válido por SSRC (`ssrc_last_toc`). `opus-rs` rechaza un
  slice vacío antes de ejecutar la ocultación, así que el PLC nunca corría.
- **Rampa de ganancia de 8 ms** por SSRC en los callbacks F32 e I16. El arranque
  y el final de un talkspurt ya no son un escalón de amplitud; la cola vacía baja
  la ganancia a cero en vez de cortar en cuatro muestras.
- **Silencio/DTX no cifrado** (`F8 FF FE`) se trata como silencio válido.
- **Fallback de descifrado DAVE** entre los usuarios del grupo y passthrough de
  paquetes que no llevan marcador DAVE cuando el emisor no está en el grupo.
- Nuevos contadores de diagnóstico: `rx_dave_plaintext`, `rx_plc_frames`,
  `rx_plc_fail`.

### 1.2. Bandeja del sistema (Linux / KDE Plasma Wayland)
- Se reemplazó libappindicator por **StatusNotifierItem vía `ksni`**. El clic
  izquierdo dispara `activate()` y **alterna** mostrar/ocultar la ventana; el
  menú contextual ofrece “Mostrar/Ocultar” y “Sair”.
- El bucle de `gtk::main_iteration` cada 80 ms se eliminó en Linux: ya no hay
  razón para despertar GTK cuando el icono lo gestiona zbus.
- Mostrar/ocultar conserva el guardado de posición, limpieza de invitaciones y
  `trim_process_memory()`.

### 1.3. Llamadas DM
- **Tono de llamada entrante** y **ringback de llamada saliente** sintetizados y
  en bucle; se detienen al aceptar, rechazar, colgar, recibir `CALL_DELETE` o
  conectar la voz.
- El modal de llamada entrante, aceptar/rechazar y el flujo OP4 ya existían y se
  mantienen.

### 1.4. Multimedia
- **Visor de imágenes (lightbox)**: clic sobre un adjunto descargado abre una
  capa a pantalla completa; clic en cualquier punto o en “✕” la cierra.
- **Avatares**: el pool de 256 slots y las descargas ya estaban bien; se limitó
  la precarga de disco a 512 URLs únicas para evitar el pico de CPU/RAM al abrir
  una cuenta con muchos canales.

### 1.5. Emojis
- El picker se reconstruye con **~130 emojis Unicode** y con **los emojis custom
  registrados por el Gateway** (`<:nombre:id>` / `<a:nombre:id>`), en filas de 12
  y con un tope de 512 custom para no inflar el modelo de UI.
- Se refresca al abrir el picker, por lo que los emojis de un servidor recién
  cargado aparecen sin reiniciar.

### 1.6. Soundboard
- El botón del panel ya invoca el motor de sonido (antes el callback no tenía
  handler en Rust).
- Se añadieron sonidos sintetizados locales y lectura de WAV custom.
- **Catalogo real de Discord**: se cargan los sonidos por defecto
  (`/soundboard-default-sounds`) y los de cada servidor
  (`/guilds/{id}/soundboard-sounds`) al iniciar sesión y al cambiar de servidor.
- Los sonidos remotos se descargan del CDN, se decodifican con un lector Ogg/Opus
  mínimo y se reproducen en local.
- Al pulsar un sonido remoto se envía **Gateway OP 31** para que el resto del
  canal también lo oiga; los efectos entrantes (`VOICE_CHANNEL_EFFECT_SEND`) se
  reproducen solo en local para no reenviarlos por el micrófono.

## 2. Verificación realizada

- `cargo check --bin litecord --offline`: **sin errores**.
- `cargo build --release --bin litecord --offline`: **correcto**.
- `cargo test --lib`: la compilación de tests llega al enlazado y falla por
  `undefined symbol: mi_collect` de mimalloc, un problema de enlace del entorno
  de test ya presente; no es un fallo del código nuevo. Los tests unitarios de
  avatar/emoji están escritos y compilan.
- No se reinició la instancia en llamada (PID 52998) para no cortar la voz.

## 2.1. Segunda tanda (feedback del usuario, 2026-11-04)

- **Avatares del sidebar/DMs vacíos:** `slot_for` adjuntaba la imagen al pool
  directamente; cuando la lista se construía en un worker de tokio, el modelo de
  Slint no era accesible y la imagen ya cacheada se perdía. Ahora se reprograma en
  el hilo de UI. También se bajó la precarga de disco de 512 a 128 avatares.
- **RAM de arranque:** la supervisión de 20 min mostró un pico inicial y luego
  estabilización en ~120–175 MB RSS. El pico es la carga de 22 servidores,
  mensajes y avatares; se recortó la precarga y se mantiene `trim_process_memory`.
- **Contacto fantasma en llamada:** ya no se registra un participante cuando el
  SSRC no está mapeado a un usuario real (antes se usaba el SSRC como uid). Se
  limpia la lista al iniciar/aceptar una llamada.
- **Segunda llamada sin tono:** el ringtone ya no pasa por la cola de sonidos de
  UI (que se queda solo con el último sonido y podía descartarlo). Se reproduce
  directamente, con estado reseteado al iniciar cada llamada.
- **Animación de llamada:** pulso de opacidad continuo mientras
  `is_voice_connecting` o una llamada DM espera participantes.
- **Botón de sonidos:** movido de la fila de emojis a la cabecera de voz, a la
  izquierda de la cámara.
- **Soundboard inaudible:** antes solo se inyectaba en el loopback del micrófono,
  que únicamente se mezcla durante la prueba de micrófono. Ahora también se
  reproduce localmente por el dispositivo de salida.
- **Mute de servidor:** se guarda el flag `mute` (distinto de `self_mute`) y la
  fila del participante muestra `SV · MUTADO` cuando lo silenció un moderador.
- **Log:** se silenciaron los INFO internos de zbus/tracing para reducir ruido y
  wakeups (KDE consulta el icono SNI cada segundo).
- **Icono del tray:** se cachea el PNG decodificado en vez de decodificarlo en
  cada `GetAll` de Plasma.

## 2.2. Context menus y roles (siguiente bloque grande)

El menú contextual de Discord requiere trabajo de RBAC real, no solo UI. Plan:

1. **Datos de roles**: parsear `guild.roles` (id, nombre, color, posición,
   permisos) y `member.roles` desde `GUILD_CREATE`/`READY_SUPPLEMENTAL`, y
   exponer `member_roles(guild, user)` y `has_permission(guild, perm)`.
2. **Menú de usuario**: perfil, mención, DM, llamada, nota, apodo, volumen,
   silenciar, silenciar soundboard, desactivar vídeo, apps, invitar, eliminar
   amigo, ignorar, bloquear, roles, mover, vista de moderador, silenciar/
   ensordecer servidor, desconectar y copiar ID. Las acciones de moderación se
   muestran/atienden según permisos (`MUTE_MEMBERS`, `MOVE_MEMBERS`,
   `DEAFEN_MEMBERS`, `KICK_MEMBERS`, `BAN_MEMBERS`).
3. **Menú de canal**: marcar leído, favorito, invitar, fijar, copiar enlace,
   silenciar, notificaciones, mover, editar, duplicar, crear, eliminar, copiar ID.
4. **Menú de servidor**: marcar leído, invitar, silenciar, notificaciones, ocultar
   canales silenciados, mostrar todos, privacidad, editar perfil, mover, salir,
   copiar ID.
5. **UI**: componente reutilizable `ContextMenu` en Slint con submenús,
   checkboxes y control deslizante de volumen; se abre con clic derecho en filas
   de usuario/canal/servidor y se cierra con clic fuera o Escape.
6. **HTTP**: endpoints de moderación (`PATCH /guilds/{g}/members/{u}`,
   `PUT/DELETE /guilds/{g}/bans/{u}`, mover a canal de voz, notas, bloqueos,
   amigos) y manejo de errores 403 por permisos.

## 2.3. Context menus y roles (primera implementación, 2026-11-04)

Ya está implementado:

- **Roles reales**: se parsean `guild.roles` (id, nombre, color, posición,
  permisos) y `member.roles` desde READY/GUILD_CREATE. Se exponen
  `member_role_names(guild, user)` y `has_guild_permission(guild, user, perm)`.
- **Menú contextual** genérico en Slint (`ContextMenuItem`) con título, iconos,
  checks, separadores, acciones deshabilitadas y cierre por clic fuera.
- **Clic derecho en filas del sidebar**: canal y miembro de voz.
- **Clic derecho en el icono de servidor**.
- **Acciones funcionales**:
  - Usuario: mención, silenciar local, volumen ±10 %, silenciar/ensordecer/
    desconectar en el servidor (solo si el rol tiene `MUTE_MEMBERS`,
    `DEAFEN_MEMBERS` o `MOVE_MEMBERS`), lista de roles y copiar ID.
  - Canal: marcar como leído, copiar ID, copiar enlace y silenciar (según
    `MANAGE_CHANNELS`).
  - Servidor: copiar ID y copiar enlace.
- **HTTP**: `PATCH /guilds/{g}/members/{u}` para moderación.

Queda para la siguiente pasada:

- Clic derecho sobre avatares/nombres en el chat y sobre las tarjetas de la sala
  de voz.
- Submenús reales (Apps, Invitar, Roles, Mover a), checkboxes y slider inline.
- Acciones destructivas con confirmación: abandonar servidor, banear, expulsar,
  eliminar/editar canal.
- Endpoints de amigos, notas, bloqueos e invitaciones.
- Mostrar color/nombre de rol junto al usuario en chat y voz.

## 2.4. Permisos, edición y moderación (2026-11-04)

- **Permisos por canal**: se guardan los `permission_overwrites` de cada canal y
  se calcula `has_channel_permission` aplicando @everyone → roles → miembro,
  con bypass de dueño/administrador.
- **Jerarquía de roles**: `can_moderate_member` exige el permiso y un rol
  superior al del objetivo; nadie puede moderar al dueño ni a un rol igual o
  superior.
- **Menú de usuario ampliado**: DM, llamada, nota, apodo de amigo, eliminar
  amigo, bloquear, silenciar soundboard, mover a canal de voz, expulsar, banear,
  silenciar/ensordecer/desconectar en servidor, roles y copiar ID.
- **Menú de canal**: editar, eliminar, crear canal de texto, copiar enlace/ID,
  marcar leído y silenciar.
- **Menú de servidor**: editar nombre, crear canal, marcar todo leído, copiar
  ID/enlace y abandonar con confirmación.
- **Modales**: diálogo de edición (nombre, tema, NSFW) y diálogo de confirmación
  para acciones destructivas.
- **HTTP nuevo**: editar/eliminar/crear canal, editar servidor, salir del
  servidor, expulsar, banear, abrir DM, notas, apodos y relaciones.
- **Soundboard**: silenciar a un usuario concreto desde el menú; sus efectos
  entrantes se ignoran.

Pendiente real (no implementado todavía):

- Submenús anidados visuales (Apps, Invitar, Ajustes de notificaciones).
- Slider de volumen dentro del menú (hoy son botones ±10 %).
- Favoritos/fijar canal/notificaciones persistidas.
- Vista de moderador y mover servidor/canal.
- Desactivar vídeo de un usuario.
- Mostrar color de rol junto al nombre en chat y voz.

## 2.5. Paridad de menús (2026-11-04, tercera pasada)

- **Bug de llamada rechazada**: `CALL_UPDATE` con `ringing=false` ya no se trata
  como llamada entrante; emite `CallEnded` y detiene el tono. Antes el rechazo
  reabría el modal y parecía aceptado.
- **Título de DM**: al abrir un DM desde el menú se fija guild `@me`, el nombre
  del destinatario y se llama a `select_channel`; ya no muestra `#canal`.
- **Submenús anidados**: panel secundario a la derecha con ítems propios
  (Notificaciones de canal/servidor, Apps). Se abre con `›`.
- **Slider de volumen**: dentro del menú. Controla volumen de usuario o, si el
  usuario está transmitiendo, el volumen del stream.
- **Persistencia**: favoritos, canales fijados, canales silenciados y modo de
  notificaciones se guardan en `~/.config/litecord/channel_prefs.json`.
- **Vista de moderador**: muestra ID y roles del miembro.
- **Mover**: servidores arriba/abajo (persistido) y canales a categoría vía
  `PATCH /channels/{id}`.
- **Desactivar vídeo de un usuario**: oculta su stream y descarta sus frames.
- **Roles visibles**: color y badge del rol más alto en la lista de voz, en la
  sala de llamada y en los mensajes del chat.
- **Menú de usuario** ampliado con notas, apodo, bloquear, eliminar amigo,
  sonido, vídeo, expulsar/banear y mover a voz.
- Clic derecho sobre la tarjeta de stream abre el menú del usuario con el slider
  de volumen del stream.

Pendiente menor: submenús de Apps/Invitaciones con datos reales, notificaciones
con duración (15m/1h/…), y acciones de apps. **Resuelto en 2.6.**

## 2.5.1. Roles reales, arrastre de miembros y comandos (cuarta pasada)

- **Roles asignables, no solo visibles**: el menú de usuario abre un submenú con
  checkboxes para asignar/quitar roles. Solo aparecen roles por debajo del rol
  más alto del actor; se excluyen `@everyone`, roles gestionados por integraciones
  y cualquier rol igual o superior. La regla se aplica también al objetivo.
- **Modelo RBAC actualizado en vivo**: `GUILD_ROLE_CREATE/UPDATE/DELETE` y
  `GUILD_MEMBER_UPDATE` actualizan el store local, en lugar de esperar a otro
  READY. Un cambio de rol refresca el color/badge de las filas de voz ya pintadas.
- **Endpoint de roles**: `PUT/DELETE /guilds/{g}/members/{u}/roles/{r}`.
- **Arrastrar miembros de voz**: arrastrar una fila de participante y soltarla
  sobre otro canal de voz, o sobre cualquiera de sus participantes, envía
  `PATCH /guilds/{g}/members/{u}` con `channel_id`. Requiere `MOVE_MEMBERS` y
  jerarquía estricta. Soltar en el mismo canal o en un canal de texto no hace nada.
- **Comandos de aplicación**: se combinan el índice del servidor y el índice
  global/de usuario (DMs incluidos), se deduplican por app/comando y se descartan
  cargas antiguas al cambiar de ámbito. El tipo de la primera opción se conserva
  para enviar enteros, booleanos, usuarios, canales, roles y números con el tipo
  correcto en la interacción.
- La ventana de comandos sigue mostrando una sola opción; los comandos con
  subcomandos anidados requieren un asistente por pasos y quedan fuera de esta
  pasada.

## 2.6. Apps, invitaciones y silencios (quinta pasada)

- **Apps reales en el menú**: el submenú Apps pide
  `GET /guilds/{id}/application-command-index`, lo cachea por servidor y muestra
  una sección por aplicación con sus comandos. Al elegir uno se inserta
  `/comando ` en el compositor para añadir argumentos y enviarlo con el flujo de
  interacciones ya existente. Maneja estado de carga y servidores sin apps.
- **Invitaciones reales**: submenú “Invitar al servidor” / “Invitar a la gente”
  en los menús de servidor y canal. Lista los servidores donde el usuario puede
  crear invitaciones (con respaldo si los roles aún no hidrataron), elige el
  primer canal de texto disponible (o consulta `GET /guilds/{id}/channels`) y
  crea la invitación con `POST /channels/{id}/invites` (24 h, usos ilimitados).
  El enlace `discord.gg/…` se copia al portapapeles.
- **Duraciones de silencio**: submenú “Silenciar con duración” con 15 min, 1 h,
  8 h, 24 h y quitar silencio. Envía `communication_disabled_until` en
  `PATCH /guilds/{g}/members/{u}` con fecha ISO-8601 en UTC.
- **Notificaciones de servidor persistidas**: usan el mismo mapa que los canales
  en `channel_prefs.json`, por lo que el modo elegido sobrevive al reinicio.
- **Silenciar servidor persistido**: el ítem del menú de servidor escribe en el
  conjunto `muted`, igual que el silencio de canal.
- Se añadió `PERM_CREATE_INSTANT_INVITE` y `create_channel_invite` en el cliente
  HTTP. También se corrigieron dos roturas de compilación de la pasada de roles
  (`managed` en `StrictReadyRole` y préstamos en `role_toggle`).

## 2.7. Pipeline de imágenes (evaluación + mejora aplicada)

La idea del libro (descargar → downscale en el worker → bitmap chico en RAM →
copia comprimida en disco) **ya estaba aplicada en parte**:

- **Avatares**: se decodifican y recortan a `40×40` **antes** de entrar a RAM
  (`AVATAR_SIZE`), y el pool de UI guarda solo esos 6,4 KB por avatar. El disco
  guarda RGBA crudo de `40×40`; a ese tamaño comprimir a WebP ahorraría disco
  pero añadiría decodificación en cada arranque, así que no compensa.
- **Emojis**: se guardan como PNG en disco y se cachean decodificados en RAM con
  LRU; el PNG ya está comprimido.
- **Adjuntos**: se decodifican y limitan a `1024×1024` antes de RAM (presupuesto
  de 12 MB). **Mejora aplicada ahora**: el disco ya no guarda la subida original
  de 1–3 MB, sino el bitmap ya reducido re-codificado como JPEG de calidad 82.
  Así el próximo arranque lee/decodifica un archivo mucho más chico y no repite
  el downscale de la imagen completa. Los archivos que no son imagen siguen
  guardándose tal cual.

Conclusión: el principio del libro es correcto y ya rige la app; el cambio real
era persistir el thumbnail comprimido de los adjuntos, no los avatares.

## 3. Pendiente / siguiente iteración

> **Recordatorio para la próxima:** completar submenús de **Apps** e
> **Invitaciones** con datos reales, duraciones de silencio (15 m / 1 h / 8 h /
> 24 h), acciones de apps instaladas y selector de servidores para invitar.

### P0 — Validación en llamada real
1. Medir 10 min de llamada y comprobar que `rx_plc_frames > 0` cuando
   `rx_dave_fail > 0` y que `rx_decode_ok != rx_dave_ok`.
2. Escuchar con pérdida sintética (0/5/10 %) y verificar que no hay saltos
   mayores de 0,2 en 1 ms.
3. Probar el clic izquierdo del tray en Plasma Wayland y el toggle con la ventana
   oculta y visible.

### P1 — Audio de tiempo real
- Sacar locks, `HashMap` y allocations del callback de audio; mezclar en un hilo
  dedicado y dejar el callback como copia de un ring buffer SPSC.
- Jitter buffer adaptativo por SSRC (40–120 ms) y reordenación por secuencia RTP.
- Contadores `ok/fail/no_magic` por SSRC y log cada 5 s.

### P1 — Soundboard
- Normalizar el volumen con el campo `volume` de la API.
- Cachear en disco los Ogg descargados y respetar `available`.
- Verificar que OP 31 usa el `source_guild_id` correcto en DMs y guilds.

### P2 — Rendimiento de arranque
- Medir RSS y tiempo hasta primer frame con `cargo build --release` + `/usr/bin/time -v`.
- Evaluar carga diferida de la lista de servidores y de los emojis custom.

### P2 — UI
- Buscador real en el picker (`emoji_search_query` sigue sin usarse).
- Indicador de “reproduciendo” en el panel de sonidos y control de volumen.
- Soporte de GIF animado en avatares/emojis custom animados.

## 4. Criterio de cierre

Se considerará cerrada la iteración cuando una llamada de 10 minutos en un canal
con al menos dos personas no presente pops audibles, el tray alterne con clic
izquierdo en KDE Wayland, el lightbox abra/cierre sin bloquear la UI y el panel de
sonidos muestre y reproduzca al menos un sonido por defecto y uno de servidor.
