//! Spanish, German, French, Russian and Japanese text for the strings that
//! Rust code picks with `i18n_ui::en_pt`. Keyed by the English text; anything
//! missing here stays in English. Machine translations that still need a
//! native-speaker review, like the other non-Portuguese tables.
//!
//! The default audio device names and "Participant" are recognized by text
//! elsewhere: use `is_default_device` and `is_placeholder_participant`, which
//! accept every language, instead of comparing against one of them.

/// `[es, de, fr, ru, ja]` for an English string, if there is one.
pub fn others(en: &str) -> Option<[&'static str; 5]> {
    Some(match en {
        "System Default Microphone" => ["Micrófono predeterminado del sistema", "Standardmikrofon des Systems", "Microphone par défaut du système", "Микрофон системы по умолчанию", "システム既定のマイク"],
        "System Default Speakers" => ["Altavoces predeterminados del sistema", "Standardlautsprecher des Systems", "Haut-parleurs par défaut du système", "Динамики системы по умолчанию", "システム既定のスピーカー"],
        "Participant" => ["Participante", "Teilnehmer", "Participant", "Участник", "参加者"],
        "@user" => ["@usuario", "@Benutzer", "@utilisateur", "@пользователь", "@ユーザー"],
        "Automatic (Recommended)" => ["Automático (recomendado)", "Automatisch (empfohlen)", "Automatique (recommandé)", "Автоматически (рекомендуется)", "自動（推奨）"],
        "Captures foreground or background windows directly through DWM." => ["Captura ventanas en primer o segundo plano directamente con DWM.", "Erfasst Fenster im Vorder- oder Hintergrund direkt über DWM.", "Capture les fenêtres au premier ou à l’arrière-plan directement via DWM.", "Захватывает окна на переднем или заднем плане напрямую через DWM.", "DWMを通じて前面または背面のウィンドウを直接キャプチャします。"],
        "Captures the default PulseAudio/PipeWire sink monitor through GStreamer pulsesrc." => ["Captura el monitor de la salida predeterminada de PulseAudio/PipeWire con GStreamer pulsesrc.", "Erfasst den Monitor der Standardausgabe von PulseAudio/PipeWire über GStreamer pulsesrc.", "Capture le moniteur de la sortie par défaut PulseAudio/PipeWire via GStreamer pulsesrc.", "Захватывает монитор стандартного выхода PulseAudio/PipeWire через GStreamer pulsesrc.", "GStreamer pulsesrcでPulseAudio/PipeWireの既定出力のモニターをキャプチャします。"],
        "Captures the system's default output device directly." => ["Captura directamente el dispositivo de salida predeterminado del sistema.", "Erfasst direkt das Standard-Ausgabegerät des Systems.", "Capture directement la sortie audio par défaut du système.", "Захватывает стандартное устройство вывода системы напрямую.", "システムの既定の出力デバイスを直接キャプチャします。"],
        "Classic high-performance capture straight from the desktop framebuffer." => ["Captura clásica de alto rendimiento directamente del framebuffer del escritorio.", "Klassische, schnelle Aufnahme direkt aus dem Desktop-Framebuffer.", "Capture classique et performante directement depuis le framebuffer du bureau.", "Классический быстрый захват прямо из кадрового буфера рабочего стола.", "デスクトップのフレームバッファから直接行う従来の高速キャプチャです。"],
        "Connection failed: the proxy refused the connection or is offline." => ["Error de conexión: el proxy rechazó la conexión o está desconectado.", "Verbindung fehlgeschlagen: Der Proxy hat die Verbindung abgelehnt oder ist offline.", "Échec de la connexion : le proxy a refusé la connexion ou est hors ligne.", "Ошибка подключения: прокси отклонил соединение или недоступен.", "接続に失敗しました。プロキシが接続を拒否したか、オフラインです。"],
        "Connection restarted" => ["Conexión reiniciada", "Verbindung neu gestartet", "Connexion redémarrée", "Соединение перезапущено", "接続を再開しました"],
        "Connects straight to Discord's servers (default)." => ["Conecta directamente con los servidores de Discord (predeterminado).", "Verbindet direkt mit den Discord-Servern (Standard).", "Se connecte directement aux serveurs de Discord (par défaut).", "Подключается напрямую к серверам Discord (по умолчанию).", "Discordのサーバーに直接接続します（既定）。"],
        "Could not load the messages" => ["No se pudieron cargar los mensajes", "Die Nachrichten konnten nicht geladen werden", "Impossible de charger les messages", "Не удалось загрузить сообщения", "メッセージを読み込めませんでした"],
        "Dedicated GStreamer pipeline attached to the PipeWire sink monitor." => ["Pipeline de GStreamer dedicado conectado al monitor de salida de PipeWire.", "Eigene GStreamer-Pipeline am Ausgabemonitor von PipeWire.", "Pipeline GStreamer dédié branché sur le moniteur de sortie PipeWire.", "Отдельный конвейер GStreamer, подключённый к монитору выхода PipeWire.", "PipeWireの出力モニターに接続した専用のGStreamerパイプラインです。"],
        "Disabled (Direct Connection)" => ["Desactivado (conexión directa)", "Deaktiviert (direkte Verbindung)", "Désactivé (connexion directe)", "Отключено (прямое подключение)", "無効（直接接続）"],
        "Friend" => ["Amigo", "Freund", "Ami", "Друг", "フレンド"],
        "GPU-accelerated pipeline through FFmpeg (VA-API, QSV, NVENC)." => ["Pipeline acelerado por GPU con FFmpeg (VA-API, QSV, NVENC).", "GPU-beschleunigte Pipeline über FFmpeg (VA-API, QSV, NVENC).", "Pipeline accéléré par le GPU via FFmpeg (VA-API, QSV, NVENC).", "Конвейер с ускорением на GPU через FFmpeg (VA-API, QSV, NVENC).", "FFmpeg（VA-API、QSV、NVENC）によるGPU支援パイプラインです。"],
        "HTTP CONNECT tunnel for restricted networks and proxies." => ["Túnel HTTP CONNECT para redes restringidas y proxies.", "HTTP-CONNECT-Tunnel für eingeschränkte Netzwerke und Proxys.", "Tunnel HTTP CONNECT pour les réseaux restreints et les proxys.", "Туннель HTTP CONNECT для ограниченных сетей и прокси.", "制限されたネットワークやプロキシ向けのHTTP CONNECTトンネルです。"],
        "Hardware-accelerated encoding on NVIDIA GeForce / RTX GPUs." => ["Codificación acelerada por hardware en GPU NVIDIA GeForce / RTX.", "Hardwarebeschleunigte Kodierung auf NVIDIA-GeForce-/RTX-GPUs.", "Encodage accéléré par le matériel sur les GPU NVIDIA GeForce / RTX.", "Аппаратное кодирование на видеокартах NVIDIA GeForce / RTX.", "NVIDIA GeForce / RTX GPUによるハードウェアエンコードです。"],
        "Keep the bot in the voice channel 24/7" => ["Mantener el bot en el canal de voz 24/7", "Den Bot rund um die Uhr im Sprachkanal halten", "Garder le bot dans le salon vocal 24h/24", "Держать бота в голосовом канале круглосуточно", "ボットをボイスチャンネルに24時間常駐させる"],
        "Legacy direct screen capture on X11 sessions." => ["Captura de pantalla directa heredada en sesiones X11.", "Ältere direkte Bildschirmaufnahme in X11-Sitzungen.", "Ancienne capture d’écran directe pour les sessions X11.", "Устаревший прямой захват экрана в сеансах X11.", "X11セッション向けの従来の直接画面キャプチャです。"],
        "Modern low-latency screen capture that works with Wayland and Flatpak." => ["Captura de pantalla moderna y de baja latencia, compatible con Wayland y Flatpak.", "Moderne Bildschirmaufnahme mit geringer Latenz, funktioniert mit Wayland und Flatpak.", "Capture d’écran moderne à faible latence, compatible Wayland et Flatpak.", "Современный захват экрана с низкой задержкой, работает с Wayland и Flatpak.", "WaylandとFlatpakで使える低遅延の最新画面キャプチャです。"],
        "Music" => ["Música", "Musik", "Musique", "Музыка", "音楽"],
        "Native capture that keeps friends' voices and Litecord sounds out of your stream." => ["Captura nativa que deja fuera de tu transmisión las voces de tus amigos y los sonidos de Litecord.", "Native Aufnahme, die Stimmen deiner Freunde und Litecord-Sounds aus deinem Stream heraushält.", "Capture native qui exclut de votre diffusion les voix de vos amis et les sons de Litecord.", "Встроенный захват без голосов друзей и звуков Litecord в вашей трансляции.", "フレンドの声やLitecordの音を配信に入れないネイティブキャプチャです。"],
        "Pause playback" => ["Pausar la reproducción", "Wiedergabe pausieren", "Mettre en pause", "Приостановить воспроизведение", "再生を一時停止"],
        "Play a song or add it to the queue" => ["Reproducir una canción o añadirla a la cola", "Einen Song abspielen oder zur Warteschlange hinzufügen", "Lire un morceau ou l’ajouter à la file", "Воспроизвести песню или добавить её в очередь", "曲を再生するかキューに追加"],
        "Prefers GPU hardware acceleration, falling back to the CPU." => ["Prefiere la aceleración por hardware de la GPU y recurre a la CPU si hace falta.", "Bevorzugt GPU-Hardwarebeschleunigung und weicht notfalls auf die CPU aus.", "Privilégie l’accélération matérielle du GPU, avec repli sur le CPU.", "Предпочитает аппаратное ускорение GPU, при необходимости переходит на CPU.", "GPUのハードウェア支援を優先し、使えない場合はCPUを使います。"],
        "Resume playback" => ["Reanudar la reproducción", "Wiedergabe fortsetzen", "Reprendre la lecture", "Продолжить воспроизведение", "再生を再開"],
        "SOCKS5 tunnel that works with Tor, Shadowsocks and SSH." => ["Túnel SOCKS5 compatible con Tor, Shadowsocks y SSH.", "SOCKS5-Tunnel, funktioniert mit Tor, Shadowsocks und SSH.", "Tunnel SOCKS5 compatible avec Tor, Shadowsocks et SSH.", "Туннель SOCKS5, работает с Tor, Shadowsocks и SSH.", "Tor、Shadowsocks、SSHで使えるSOCKS5トンネルです。"],
        "Show the current queue" => ["Ver la cola actual", "Aktuelle Warteschlange anzeigen", "Afficher la file actuelle", "Показать текущую очередь", "現在のキューを表示"],
        "Show the song playing now" => ["Mostrar la canción que suena ahora", "Den aktuell laufenden Song anzeigen", "Afficher le morceau en cours", "Показать текущую песню", "再生中の曲を表示"],
        "Skip to the next song" => ["Pasar a la siguiente canción", "Zum nächsten Song springen", "Passer au morceau suivant", "Перейти к следующей песне", "次の曲へスキップ"],
        "Song name or YouTube/Spotify link" => ["Nombre de la canción o enlace de YouTube/Spotify", "Songname oder YouTube-/Spotify-Link", "Nom du morceau ou lien YouTube/Spotify", "Название песни или ссылка на YouTube/Spotify", "曲名またはYouTube/Spotifyのリンク"],
        "Standard capture through the CPAL audio interface." => ["Captura estándar con la interfaz de audio CPAL.", "Standardaufnahme über die CPAL-Audioschnittstelle.", "Capture standard via l’interface audio CPAL.", "Стандартный захват через аудиоинтерфейс CPAL.", "CPALオーディオインターフェースによる標準キャプチャです。"],
        "Stop playback and clear the queue" => ["Detener la reproducción y vaciar la cola", "Wiedergabe stoppen und Warteschlange leeren", "Arrêter la lecture et vider la file", "Остановить воспроизведение и очистить очередь", "再生を停止してキューを空にする"],
        "System Proxy" => ["Proxy del sistema", "System-Proxy", "Proxy du système", "Системный прокси", "システムプロキシ"],
        "This channel is empty or has no recent messages." => ["Este canal está vacío o no tiene mensajes recientes.", "Dieser Kanal ist leer oder hat keine neuen Nachrichten.", "Ce salon est vide ou n’a pas de messages récents.", "Этот канал пуст или в нём нет недавних сообщений.", "このチャンネルは空か、最近のメッセージがありません。"],
        "This server has no text channels your account can access." => ["Este servidor no tiene canales de texto a los que tu cuenta pueda acceder.", "Dieser Server hat keine Textkanäle, auf die dein Konto zugreifen kann.", "Ce serveur n’a aucun salon textuel accessible à votre compte.", "На этом сервере нет текстовых каналов, доступных вашей учётной записи.", "このサーバーには、あなたのアカウントで見られるテキストチャンネルがありません。"],
        "Tries PrintWindow for DirectX windows and uses GDI BitBlt for the desktop and as fallback." => ["Prueba PrintWindow en ventanas DirectX y usa GDI BitBlt para el escritorio y como alternativa.", "Versucht PrintWindow für DirectX-Fenster und nutzt GDI BitBlt für den Desktop und als Ausweichlösung.", "Essaie PrintWindow pour les fenêtres DirectX et utilise GDI BitBlt pour le bureau et en secours.", "Пробует PrintWindow для окон DirectX и использует GDI BitBlt для рабочего стола и как запасной вариант.", "DirectXウィンドウにはPrintWindowを試し、デスクトップや代替手段にはGDI BitBltを使います。"],
        "Universal multithreaded software encoding with SIMD optimizations." => ["Codificación universal por software, multihilo y con optimizaciones SIMD.", "Universelle Software-Kodierung mit mehreren Threads und SIMD-Optimierungen.", "Encodage logiciel universel, multithread et optimisé SIMD.", "Универсальное многопоточное программное кодирование с оптимизациями SIMD.", "SIMD最適化付きのマルチスレッド・ソフトウェアエンコードです。"],
        "Update failed" => ["Error en la actualización", "Aktualisierung fehlgeschlagen", "Échec de la mise à jour", "Ошибка обновления", "アップデートに失敗しました"],
        "Uses the system's own picker through XDG Desktop Portal and PipeWire." => ["Usa el selector del propio sistema con XDG Desktop Portal y PipeWire.", "Nutzt die systemeigene Auswahl über XDG Desktop Portal und PipeWire.", "Utilise le sélecteur du système via XDG Desktop Portal et PipeWire.", "Использует системный выбор через XDG Desktop Portal и PipeWire.", "XDG Desktop PortalとPipeWireでシステムの選択画面を使います。"],
        "Uses the system-wide proxy automatically." => ["Usa automáticamente el proxy del sistema.", "Verwendet automatisch den systemweiten Proxy.", "Utilise automatiquement le proxy du système.", "Автоматически использует системный прокси.", "システム全体のプロキシを自動で使います。"],
        "Video" => ["Video", "Video", "Vidéo", "Видео", "動画"],
        "WASAPI process-isolated capture (keeps Litecord out of the stream audio)." => ["Captura WASAPI aislada por proceso (deja a Litecord fuera del audio de la transmisión).", "WASAPI-Aufnahme mit Prozessisolierung (hält Litecord aus dem Stream-Audio heraus).", "Capture WASAPI isolée par processus (exclut Litecord du son de la diffusion).", "Захват WASAPI с изоляцией процесса (без звука Litecord в трансляции).", "プロセス分離のWASAPIキャプチャ（配信音声にLitecordの音を入れません）。"],
        "Watch Video" => ["Ver video", "Video ansehen", "Regarder la vidéo", "Смотреть видео", "動画を見る"],
        "You" => ["Tú", "Du", "Vous", "Вы", "あなた"],
        "[A new member joined the server!]" => ["[¡Un nuevo miembro se unió al servidor!]", "[Ein neues Mitglied ist dem Server beigetreten!]", "[Un nouveau membre a rejoint le serveur !]", "[Новый участник присоединился к серверу!]", "[新しいメンバーがサーバーに参加しました！]"],
        "[App subscription activated]" => ["[Suscripción de aplicación activada]", "[App-Abonnement aktiviert]", "[Abonnement d’application activé]", "[Подписка на приложение активирована]", "[アプリのサブスクリプションが有効になりました]"],
        "[AutoMod action]" => ["[Acción de AutoMod]", "[AutoMod-Aktion]", "[Action d’AutoMod]", "[Действие AutoMod]", "[AutoModのアクション]"],
        "[Channel icon changed]" => ["[Icono del canal cambiado]", "[Kanalsymbol geändert]", "[Icône du salon modifiée]", "[Значок канала изменён]", "[チャンネルのアイコンが変更されました]"],
        "[Channel integration added]" => ["[Integración de canal añadida]", "[Kanalintegration hinzugefügt]", "[Intégration de salon ajoutée]", "[Интеграция канала добавлена]", "[チャンネル連携が追加されました]"],
        "[Channel integration removed]" => ["[Integración de canal eliminada]", "[Kanalintegration entfernt]", "[Intégration de salon supprimée]", "[Интеграция канала удалена]", "[チャンネル連携が削除されました]"],
        "[Channel name changed]" => ["[Nombre del canal cambiado]", "[Kanalname geändert]", "[Nom du salon modifié]", "[Название канала изменено]", "[チャンネル名が変更されました]"],
        "[Code Block" => ["[Bloque de código", "[Codeblock", "[Bloc de code", "[Блок кода", "[コードブロック"],
        "[Code Block]" => ["[Bloque de código]", "[Codeblock]", "[Bloc de code]", "[Блок кода]", "[コードブロック]"],
        "[Followed channel added]" => ["[Canal seguido añadido]", "[Gefolgter Kanal hinzugefügt]", "[Salon suivi ajouté]", "[Добавлен отслеживаемый канал]", "[フォロー中のチャンネルが追加されました]"],
        "[Forwarded message]" => ["[Mensaje reenviado]", "[Weitergeleitete Nachricht]", "[Message transféré]", "[Пересланное сообщение]", "[転送されたメッセージ]"],
        "[Invite reminder]" => ["[Recordatorio de invitación]", "[Einladungserinnerung]", "[Rappel d’invitation]", "[Напоминание о приглашении]", "[招待のリマインダー]"],
        "[Member added to the group]" => ["[Miembro añadido al grupo]", "[Mitglied zur Gruppe hinzugefügt]", "[Membre ajouté au groupe]", "[Участник добавлен в группу]", "[メンバーがグループに追加されました]"],
        "[Member removed from the group]" => ["[Miembro eliminado del grupo]", "[Mitglied aus der Gruppe entfernt]", "[Membre retiré du groupe]", "[Участник удалён из группы]", "[メンバーがグループから削除されました]"],
        "[Message pinned]" => ["[Mensaje fijado]", "[Nachricht angeheftet]", "[Message épinglé]", "[Сообщение закреплено]", "[メッセージがピン留めされました]"],
        "[New thread created]" => ["[Nuevo hilo creado]", "[Neuer Thread erstellt]", "[Nouveau fil créé]", "[Создана новая ветка]", "[新しいスレッドが作成されました]"],
        "[Nitro offer / upgrade]" => ["[Oferta / mejora de Nitro]", "[Nitro-Angebot / Upgrade]", "[Offre / mise à niveau Nitro]", "[Предложение / улучшение Nitro]", "[Nitroのオファー / アップグレード]"],
        "[Poll result]" => ["[Resultado de la encuesta]", "[Umfrageergebnis]", "[Résultat du sondage]", "[Результат опроса]", "[投票結果]"],
        "[Role subscription purchase]" => ["[Compra de suscripción de rol]", "[Kauf eines Rollenabonnements]", "[Achat d’abonnement de rôle]", "[Покупка подписки на роль]", "[ロールサブスクリプションの購入]"],
        "[Server back in Discovery]" => ["[El servidor vuelve a Descubrir]", "[Server wieder in Entdecken]", "[Serveur de retour dans Découverte]", "[Сервер снова в каталоге]", "[サーバーが「見つける」に戻りました]"],
        "[Server boosted!]" => ["[¡Servidor mejorado!]", "[Server geboostet!]", "[Serveur boosté !]", "[Сервер буст!]", "[サーバーがブーストされました！]"],
        "[Server purchase]" => ["[Compra en el servidor]", "[Kauf auf dem Server]", "[Achat sur le serveur]", "[Покупка на сервере]", "[サーバーでの購入]"],
        "[Server reached boost level 1!]" => ["[¡El servidor alcanzó el nivel 1 de mejora!]", "[Server hat Boost-Stufe 1 erreicht!]", "[Le serveur a atteint le niveau de boost 1 !]", "[Сервер достиг 1-го уровня буста!]", "[サーバーがブーストレベル1に到達しました！]"],
        "[Server reached boost level 2!]" => ["[¡El servidor alcanzó el nivel 2 de mejora!]", "[Server hat Boost-Stufe 2 erreicht!]", "[Le serveur a atteint le niveau de boost 2 !]", "[Сервер достиг 2-го уровня буста!]", "[サーバーがブーストレベル2に到達しました！]"],
        "[Server reached boost level 3!]" => ["[¡El servidor alcanzó el nivel 3 de mejora!]", "[Server hat Boost-Stufe 3 erreicht!]", "[Le serveur a atteint le niveau de boost 3 !]", "[Сервер достиг 3-го уровня буста!]", "[サーバーがブーストレベル3に到達しました！]"],
        "[Server removed from Discovery]" => ["[Servidor retirado de Descubrir]", "[Server aus Entdecken entfernt]", "[Serveur retiré de Découverte]", "[Сервер удалён из каталога]", "[サーバーが「見つける」から外されました]"],
        "[Server safety alert]" => ["[Alerta de seguridad del servidor]", "[Sicherheitswarnung des Servers]", "[Alerte de sécurité du serveur]", "[Предупреждение безопасности сервера]", "[サーバーの安全アラート]"],
        "[Stage started/updated]" => ["[Escenario iniciado/actualizado]", "[Stage gestartet/aktualisiert]", "[Conférence lancée/mise à jour]", "[Трибуна начата/обновлена]", "[ステージが開始/更新されました]"],
        "[Thread/forum starter message]" => ["[Mensaje inicial del hilo/foro]", "[Startnachricht des Threads/Forums]", "[Message de départ du fil/forum]", "[Первое сообщение ветки/форума]", "[スレッド/フォーラムの最初のメッセージ]"],
        "[Voice call started/ended]" => ["[Llamada de voz iniciada/finalizada]", "[Sprachanruf gestartet/beendet]", "[Appel vocal démarré/terminé]", "[Голосовой звонок начат/завершён]", "[ボイス通話が開始/終了しました]"],
        "🔴 Disconnected from the voice channel." => ["🔴 Desconectado del canal de voz.", "🔴 Vom Sprachkanal getrennt.", "🔴 Déconnecté du salon vocal.", "🔴 Отключено от голосового канала.", "🔴 ボイスチャンネルから切断しました。"],
        "Private Channel
This channel is restricted and needs specific server roles to view its messages." => ["Canal privado
Este canal está restringido y requiere roles específicos del servidor para ver sus mensajes.", "Privater Kanal
Dieser Kanal ist eingeschränkt; zum Lesen der Nachrichten brauchst du bestimmte Serverrollen.", "Salon privé
Ce salon est restreint et nécessite des rôles précis sur le serveur pour voir ses messages.", "Закрытый канал
Этот канал ограничен: для просмотра сообщений нужны определённые роли на сервере.", "プライベートチャンネル
このチャンネルは制限されており、メッセージを見るにはサーバーの特定のロールが必要です。"],
        _ => return None,
    })
}

/// Every language's text for an English string, Portuguese included.
fn all_texts(en: &'static str, pt: &'static str) -> impl Iterator<Item = &'static str> {
    [en, pt].into_iter().chain(others(en).into_iter().flatten())
}

/// True for an empty name or the "system default" device name in any
/// language, so a choice saved in one language still works in another.
pub fn is_default_device(name: &str) -> bool {
    name.is_empty()
        || name.contains("Padrão")
        || all_texts("System Default Microphone", "Microfone Padrão do Sistema")
            .chain(all_texts("System Default Speakers", "Alto-falantes Padrão do Sistema"))
            .any(|t| t == name)
}

/// True for the "Participant #id" placeholder in any language.
pub fn is_placeholder_participant(name: &str) -> bool {
    all_texts("Participant", "Participante").any(|t| {
        name.strip_prefix(t).is_some_and(|rest| rest.starts_with(" #"))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_device_and_participant_match_every_language() {
        for n in ["", "System Default Speakers", "Microfone Padrão do Sistema", "Standardmikrofon des Systems", "システム既定のスピーカー"] {
            assert!(super::is_default_device(n), "{n}");
        }
        assert!(!super::is_default_device("Realtek Speakers"));
        for n in ["Participant #1", "Participante #2", "Teilnehmer #3", "Участник #4", "参加者 #5"] {
            assert!(super::is_placeholder_participant(n), "{n}");
        }
        assert!(!super::is_placeholder_participant("Participant"));
        assert!(!super::is_placeholder_participant("Henrique"));
    }

    /// Every English string passed to `en_pt` has the other languages, except
    /// the ones other code matches by text.
    #[test]
    fn every_en_pt_string_is_translated() {
        let skip: [&str; 0] = [];
        let mut missing = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for part in text.split("en_pt(\"").skip(1) {
                    let en = part.split("\", \"").next().unwrap().replace("\\n", "\n");
                    if !skip.contains(&en.as_str()) && super::others(&en).is_none() && !en.contains('\\') {
                        missing.push(format!("{}: {}", path.display(), en));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "missing translations: {missing:#?}");
    }
}
