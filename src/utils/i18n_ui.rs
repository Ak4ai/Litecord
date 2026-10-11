//! UI strings that are not part of the older `i18n::Translations` table.
//!
//! English and Portuguese are complete; the other languages fall back to English
//! until they get their own table. The values are pushed into the Slint `Tr` global.

use std::sync::atomic::{AtomicU8, Ordering};

use super::i18n::Language;

macro_rules! ui_strings {
    ($(($key:ident, $setter:ident, $en:expr, $pt:expr)),* $(,)?) => {
        pub struct UiStrings {
            $(pub $key: &'static str,)*
        }

        pub const EN_UI: UiStrings = UiStrings { $($key: $en,)* };
        pub const PT_UI: UiStrings = UiStrings { $($key: $pt,)* };

        /// (key, English, Portuguese) for every string; used by the tests.
        #[cfg(test)]
        pub fn entries() -> Vec<(&'static str, &'static str, &'static str)> {
            vec![$((stringify!($key), EN_UI.$key, PT_UI.$key),)*]
        }

        /// Pushes every string of `s` into the Slint `Tr` global.
        pub fn apply_to_window(ui: &crate::AppWindow, s: &UiStrings) {
            use slint::ComponentHandle;
            let g = ui.global::<crate::Tr>();
            $(g.$setter(s.$key.into());)*
        }
    };
}

ui_strings! {
    (qr_generate, set_qr_generate, "Generate QR", "Gerar QR"),
    (qr_regenerate, set_qr_regenerate, "Re-generate QR Code", "Re-gerar QR Code"),
    (preview_reduced, set_preview_reduced, "Reduced preview to save resources", "Prévia reduzida para poupar recursos"),
    (you_are_streaming, set_you_are_streaming, "You are streaming", "Você está transmitindo"),
    (detached_badge, set_detached_badge, "DETACHED", "DESANEXADO"),
    (live_badge, set_live_badge, "LIVE", "AO VIVO"),
    (disable_intangible, set_disable_intangible, "Disable Intangible", "Desativar Intangível"),
    (smart_commands, set_smart_commands, "DISCORD SMART COMMANDS", "COMANDOS INTELIGENTES DO DISCORD"),
    (click_autocomplete, set_click_autocomplete, "Click to autocomplete", "Clique para autocompletar"),
    (manage_accounts, set_manage_accounts, "Manage Accounts", "Gerenciar Contas"),
    (manage_accounts_desc, set_manage_accounts_desc, "Quickly switch accounts or add a second saved account.", "Troque rapidamente de conta ou adicione uma segunda conta salva."),
    (account_connected, set_account_connected, "● Connected", "● Conectada"),
    (account_sign_in, set_account_sign_in, "Sign in", "Entrar"),
    (add_account, set_add_account, "+ Add Another Account", "+ Adicionar Outra Conta"),
    (sign_out_all, set_sign_out_all, "Sign Out of All Accounts", "Sair de Todas as Contas"),
    (update_available, set_update_available, "New Update Available!", "Nova Atualização Disponível!"),
    (dont_show_again, set_dont_show_again, "Don't show again", "Não mostrar mais"),
    (remind_later, set_remind_later, "Remind Me Later", "Lembrar Depois"),
    (yes_update, set_yes_update, "Yes, Update", "Sim, Atualizar"),
    (share_your_screen, set_share_your_screen, "Share your screen", "Compartilhe sua tela"),
    (screens_windows, set_screens_windows, "Screens / Windows", "Telas / Janelas"),
    (screens, set_screens, "Screens", "Telas"),
    (windows_tab, set_windows_tab, "Windows", "Janelas"),
    (camera, set_camera, "Camera", "Câmera"),
    (no_camera, set_no_camera, "No camera / webcam detected on the system.", "Nenhuma câmera / webcam detectada no sistema."),
    (stream_quality, set_stream_quality, "STREAM QUALITY", "QUALIDADE DA TRANSMISSÃO"),
    (resolution, set_resolution, "RESOLUTION", "RESOLUÇÃO"),
    (frame_rate, set_frame_rate, "FRAME RATE (FPS)", "TAXA DE QUADROS (FPS)"),
    (webcam_native, set_webcam_native, "The webcam will stream using your device's native resolution and frame rate.", "A webcam transmitirá utilizando a resolução e taxa nativas do seu dispositivo."),
    (stream_pc_audio, set_stream_pc_audio, "Stream Computer / Window Audio", "Transmitir Áudio do Computador / Janela"),
    (stream_pc_audio_desc, set_stream_pc_audio_desc, "Includes the app or game sound during the stream", "Inclui o som do aplicativo ou jogo durante a transmissão"),
    (browser_stream, set_browser_stream, "Streaming Browser (Videos / DRM)", "Navegador para Transmissão (Vídeos / DRM)"),
    (browser_stream_desc, set_browser_stream_desc, "Opens the browser without black-screen locks to watch with friends", "Abre o navegador sem travas de tela preta para assistir com amigos"),
    (open, set_open, "Open", "Abrir"),
    (direct_camera, set_direct_camera, "Direct Camera Stream", "Transmissão Direta de Câmera"),
    (direct_camera_desc, set_direct_camera_desc, "Webcam video only (no system audio capture)", "Apenas vídeo da webcam (sem captura de áudio do sistema)"),
    (cancel, set_cancel, "Cancel", "Cancelar"),
    (start_stream, set_start_stream, "Stream", "Transmitir"),
    (incoming_call, set_incoming_call, "Incoming voice call", "Chamada de voz recebida"),
    (answer, set_answer, "Answer", "Atender"),
    (decline, set_decline, "Decline", "Recusar"),
    (detached_title, set_detached_title, "Litecord - Detached Stream", "Litecord - Transmissão Desanexada"),
    (settings_header, set_settings_header, "SETTINGS", "CONFIGURAÇÕES"),
    (output_device, set_output_device, "Output Device", "Dispositivo de Saída"),
    (output_hint, set_output_hint, "For the best positional fidelity and to avoid echo or sound leaking in group calls, use stereo headphones.", "Para obter a melhor fidelidade posicional e evitar eco ou vazamento de som nas chamadas em grupo, utilize fones de ouvido estéreo."),
    (input_device, set_input_device, "Input Device", "Dispositivo de Entrada"),
    (input_hint, set_input_hint, "Select your main microphone or headset. We recommend using the default device or a dedicated noise-cancelling microphone.", "Selecione o microfone principal ou headset. Recomendamos usar o dispositivo padrão ou o microfone dedicado com cancelamento de ruído."),
    (input_volume_title, set_input_volume_title, "INPUT VOLUME (MICROPHONE GAIN)", "VOLUME DE ENTRADA (GANHO DO MICROFONE)"),
    (mic_gain, set_mic_gain, "Microphone Gain", "Ganho do Microfone"),
    (mic_gain_desc, set_mic_gain_desc, "Adjust your microphone gain. Values above 100% (up to 200%) amplify your voice signal for the other users in the room.", "Ajuste o ganho do seu microfone. Valores acima de 100% (até 200%) amplificam o sinal da sua voz para os outros usuários na sala."),
    (sensitivity, set_sensitivity, "Sensitivity (VAD)", "Sensibilidade (VAD)"),
    (sensitivity_desc, set_sensitivity_desc, "Speak normally. Adjust the control so your voice goes past the white mark, while noises such as a mechanical keyboard and breathing stay below it.", "Fale normalmente. Ajuste o controle para que sua voz ultrapasse a marca branca, mas ruídos como teclado mecânico e respiração fiquem abaixo."),
    (loopback_test, set_loopback_test, "Loopback Test", "Teste de Retorno (Loopback)"),
    (loopback_test_desc, set_loopback_test_desc, "Lets you hear your processed voice in real time with Litecord's filter through your headphones to calibrate volume and clarity before matches.", "Permite escutar a sua voz tratada em tempo real com o filtro do Litecord nos seus fones para calibrar volume e clareza antes das partidas."),
    (ducking, set_ducking, "Smart Voice Ducking", "Atenuação de Voz Inteligente (Ducking)"),
    (ducking_active, set_ducking_active, "ACTIVE (P:1 to P:5)", "ATIVO (P:1 a P:5)"),
    (ducking_desc, set_ducking_desc, "The Opus PLC audio engine automatically lowers other members' sound by 12 dB when priority tactical leaders speak.", "O motor de áudio Opus PLC reduz automaticamente em 12dB o som dos outros membros quando líderes táticos com prioridade falam."),
    (competitive_edge, set_competitive_edge, "Competitive Edge", "Vantagem Competitiva"),
    (competitive_edge_desc, set_competitive_edge_desc, "Eliminates the cacophony in decisive FPS/MOBA moments, making sure critical callouts stand out instantly.", "Elimina a cacofonia em momentos decisivos de jogos FPS/MOBA, garantindo que callouts críticos se sobressaiam instantaneamente."),
    (select_language, set_select_language, "SELECT THE CLIENT LANGUAGE", "SELECIONE O IDIOMA DO CLIENTE"),
    (instant_switch, set_instant_switch, "Instant Switch", "Troca Instantânea"),
    (instant_switch_desc, set_instant_switch_desc, "Language changes are applied in real time across all windows without restarting Litecord.", "A alteração de idioma é aplicada em tempo real em todas as janelas sem necessidade de reiniciar o Litecord."),
    (auto_detect, set_auto_detect, "Automatic Detection", "Detecção Automática"),
    (auto_detect_desc, set_auto_detect_desc, "The 'Auto' option detects the default region and language of your operating system (Windows/Linux).", "A opção 'Auto' identifica a região e linguagem padrão do seu sistema operacional (Windows/Linux)."),
    (keys_tip, set_keys_tip, "Key Tip", "Dica de Teclas"),
    (keys_tip_desc, set_keys_tip_desc, "You can use keys such as F1-F12, NumPad keys, or combinations with Ctrl, Shift and Alt to avoid conflicts.", "Você pode usar teclas como F1-F12, teclas do NumPad, ou combinações com Ctrl, Shift e Alt para evitar conflitos."),
    (encoder_title, set_encoder_title, "VIDEO ENCODER (H.264 ENCODER)", "CODIFICADOR DE VÍDEO (H.264 ENCODER)"),
    (encoder_desc, set_encoder_desc, "Select the GPU or CPU acceleration engine used to encode screen streams.", "Selecione o motor de aceleração por GPU ou CPU para codificar as transmissões de tela."),
    (appearance, set_appearance, "APPEARANCE", "APARÊNCIA"),
    (light_theme, set_light_theme, "Light theme", "Tema claro"),
    (light_theme_desc, set_light_theme_desc, "Switches the whole interface between the light and dark palettes.", "Alterna toda a interface entre as paletas clara e escura."),
    (capture_method, set_capture_method, "SCREEN AND WINDOW CAPTURE METHOD", "MÉTODO DE CAPTURA DE TELA E JANELAS"),
    (capture_method_desc, set_capture_method_desc, "Defines how Litecord captures the visual content of the operating system.", "Define como o Litecord captura o conteúdo visual do sistema operacional."),
    (loopback_audio_title, set_loopback_audio_title, "STREAM AUDIO (SYSTEM LOOPBACK)", "ÁUDIO DA TRANSMISSÃO (LOOPBACK DO SISTEMA)"),
    (loopback_audio_desc, set_loopback_audio_desc, "Defines the technology used to capture the internal sound of games, videos and the system.", "Define a tecnologia de captura do som interno de jogos, vídeos e do sistema."),
    (stream_ui_title, set_stream_ui_title, "STREAM NOTICES AND INTERFACE", "AVISOS E INTERFACE DE TRANSMISSÃO"),
    (economy_notice, set_economy_notice, "Reduced Preview Notice", "Aviso de Prévia Econômica"),
    (economy_notice_desc, set_economy_notice_desc, "Shows the warning pill in your own preview when the stream starts, telling you the frame rate is reduced to save resources.", "Exibe a pílula de aviso na pré-visualização própria ao iniciar a transmissão informando sobre a taxa de quadros reduzida para economizar recursos."),
    (perf_monitor, set_perf_monitor, "Performance Monitor (CPU / RAM / GPU)", "Monitor de Desempenho (CPU / RAM / GPU)"),
    (perf_monitor_desc, set_perf_monitor_desc, "Shows live RAM, CPU and GPU usage in the application title bar next to the settings.", "Exibe em tempo real o percentual de uso de RAM, CPU e GPU na barra de título do aplicativo ao lado das configurações."),
    (e2ee_title, set_e2ee_title, "E2EE Video & Sound Encryption", "Criptografia E2EE de Vídeo & Som"),
    (aes_active, set_aes_active, "AES-256-GCM ACTIVE", "AES-256-GCM ATIVO"),
    (e2ee_desc, set_e2ee_desc, "Every 60 FPS frame and the loopback audio are encrypted with 256-bit symmetric keys using AES-NI hardware acceleration.", "Cada quadro a 60 FPS e áudio loopback é criptografado com chaves simétricas de 256 bits via aceleração de hardware AES-NI."),
    (lan_protect, set_lan_protect, "Wi-Fi & LAN Protection", "Proteção Wi-Fi & LAN"),
    (lan_protect_desc, set_lan_protect_desc, "Prevents anyone on the same router from capturing your stream with packet analyzers such as Wireshark.", "Impede que qualquer pessoa no mesmo roteador capture seu stream com analisadores de pacotes como Wireshark."),
    (vault_title, set_vault_title, "Windows DPAPI Credential Vault", "Cofre de Credenciais Windows DPAPI"),
    (protected, set_protected, "PROTECTED", "PROTEGIDO"),
    (vault_desc, set_vault_desc, "Your Discord token is stored with native CryptProtectData encryption bound to your Windows user account.", "Seu token do Discord é armazenado com criptografia nativa CryptProtectData vinculada à sua conta de usuário do Windows."),
    (stealer_immunity, set_stealer_immunity, "Stealer Immunity", "Imunidade a Stealers"),
    (stealer_immunity_desc, set_stealer_immunity_desc, "Unauthorized scripts and malware cannot steal your session because the cryptographic key lives in the Windows kernel.", "Scripts e malwares não autorizados não conseguem roubar sua sessão porque a chave criptográfica reside no kernel do Windows."),
    (dave_title, set_dave_title, "Official Discord Voice (DAVE Protocol)", "Voz Discord Oficial (Protocolo DAVE)"),
    (dave_desc, set_dave_desc, "End-to-end encrypted voice communication with Discord's official voice servers, with no intermediaries.", "Comunicação de voz criptografada ponta a ponta com os servidores de voz oficiais do Discord sem intermediários."),
    (zero_intermediaries, set_zero_intermediaries, "Zero Intermediaries", "Zero Intermediários"),
    (zero_intermediaries_desc, set_zero_intermediaries_desc, "Connections are 100% direct to the Discord API. There are no third-party servers or built-in telemetry.", "Conexões 100% diretas com a API do Discord. Não existem servidores de terceiros ou telemetria embutida."),
    (proxy_mode, set_proxy_mode, "Proxy Connection Mode", "Modo de Conexão Proxy"),
    (proxy_address, set_proxy_address, "Proxy Address and Authentication", "Endereço e Autenticação do Proxy"),
    (proxy_host, set_proxy_host, "Server / Host IP:", "Servidor / IP Host:"),
    (proxy_port, set_proxy_port, "Port:", "Porta:"),
    (proxy_user, set_proxy_user, "User (Optional):", "Usuário (Opcional):"),
    (proxy_pass, set_proxy_pass, "Password (Optional):", "Senha (Opcional):"),
    (save_apply, set_save_apply, "Save and Apply", "Salvar e Aplicar"),
    (bypass_blocks, set_bypass_blocks, "Bypass Blocks", "Burlar Bloqueios"),
    (bypass_blocks_desc, set_bypass_blocks_desc, "Lets you get around firewalls and DNS blocks at schools or companies.", "Permite contornar firewalls e bloqueios de DNS em colégios ou empresas."),
    (tor_desc, set_tor_desc, "Compatible with Tor's port 9050 and local SSH tunnels to anonymize your IP.", "Compatível com a porta 9050 do Tor e túneis SSH locais para anonimizar seu IP."),
    (debug_desc, set_debug_desc, "Point to 127.0.0.1:8080 to inspect REST API requests.", "Aponte para 127.0.0.1:8080 para inspecionar requisições da API REST."),
    (gaming_perf, set_gaming_perf, "Gaming Performance", "Desempenho em Jogos"),
    (gaming_perf_desc, set_gaming_perf_desc, "With the proxy disabled, the impact is 0%. Voice and video keep direct UDP routes.", "Com proxy desativado, o impacto é 0%. Voz e vídeo mantêm rotas UDP diretas."),
    (release_channel, set_release_channel, "Official Release Channel", "Canal Oficial de Releases"),
    (release_channel_desc, set_release_channel_desc, "Updates are checked directly in the GitHub repository with domain signature validation.", "As atualizações são verificadas diretamente no repositório do GitHub com validação de assinatura de domínio."),
    (rust_perf, set_rust_perf, "Native Rust Performance", "Performance Nativa em Rust"),
    (rust_perf_desc, set_rust_perf_desc, "• CPU usage: < 0.1% when idle (0.0% in DeepSleep mode in the tray)\n• RAM: ~32 MB (versus over 700 MB for Electron)\n• Opus PLC audio pipeline with smart volume ducking", "• Uso de CPU: < 0.1% em idle (0.0% no modo DeepSleep na bandeja)\n• Memória RAM: ~32 MB (contra mais de 700 MB do Electron)\n• Pipeline de áudio Opus PLC com atenuação de volume inteligente"),
    (deepsleep, set_deepsleep, "DeepSleep Mode", "Modo DeepSleep"),
    (deepsleep_desc, set_deepsleep_desc, "When minimized to the tray, the interface freezes its rendering to guarantee 100% FPS for your games.", "Ao minimizar para a bandeja, a interface congela sua renderização para garantir 100% de FPS para seus jogos."),
    (used_command, set_used_command, "used", "usou"),
    (settings_tab_video, set_settings_tab_video, "Video & Interface", "Vídeo e Interface"),
    (stop_stream, set_stop_stream, "Stop Streaming", "Parar Transmissão"),
    (watching_stream, set_watching_stream, "Watching Stream", "Assistindo Transmissão"),
    (stream_word, set_stream_word, "Stream", "Transmissão"),
    (waiting_token, set_waiting_token, "Waiting for Access Token...", "Aguardando Token de Acesso..."),
    (not_connected, set_not_connected, "Not connected", "Não conectado"),
    (select_channel, set_select_channel, "Select a channel", "Selecione um canal"),
    (update_downloading, set_update_downloading, "Downloading update...", "Baixando atualização..."),
    (update_comparing, set_update_comparing, "Comparing local version with GitHub...", "Comparando versão local com o GitHub..."),
    (proxy_saved, set_proxy_saved, "Proxy settings saved successfully!", "Configurações de proxy salvas com sucesso!"),
    (proxy_testing, set_proxy_testing, "Testing connection through proxy...", "Testando conexão via proxy..."),
    (qr_expired, set_qr_expired, "The QR Code session expired or was cancelled on the phone. Click Generate QR to try again.", "Sessão do QR Code expirou ou foi cancelada no celular. Clique em Gerar QR para tentar novamente."),
    (qr_captcha, set_qr_captcha, "Discord required a Captcha verification for the QR Code. Please paste your Token next to it to connect right away.", "O Discord exigiu verificação por Captcha para o QR Code. Por favor, cole seu Token ao lado para conectar imediatamente."),
    (signed_out, set_signed_out, "You signed out. Enter your token or scan the QR Code.", "Você saiu da conta. Insira seu token ou escaneie o QR Code."),
    (proxy_failed, set_proxy_failed, "Connection failed via Proxy ({})", "Falha de conexão via Proxy ({})"),
    (auto_connecting, set_auto_connecting, "Auto-connecting to Gateway v9...", "Autoconectando à Gateway v9..."),
    (perms_unavailable, set_perms_unavailable, "Permissions unavailable", "Permissões indisponíveis"),
    (perms_failed, set_perms_failed, "Failed to check permissions. Select the server again to try.", "Falha ao verificar permissões. Selecione o servidor novamente para tentar."),
    (no_text_channel, set_no_text_channel, "No accessible text channel", "Nenhum canal de texto acessível"),
    (press_enter_run, set_press_enter_run, "Press Enter to run ", "Pressione Enter para executar "),
    (type_value_for, set_type_value_for, "Type the value for ", "Digite o valor para "),
    (press_enter_or_send, set_press_enter_or_send, "Press Enter or Send to run ", "Pressione Enter ou Enviar para executar "),
    (now_label, set_now_label, "Now", "Agora"),
    (welcome_msg, set_welcome_msg, "Welcome to Litecord! Your servers will load in the sidebar on the left.", "Bem-vindo ao Litecord! Seus servidores carregarão na barra lateral à esquerda."),
    (label_muted, set_label_muted, "Muted", "Mudo"),
    (label_transmitting, set_label_transmitting, "Transmitting...", "Transmitindo..."),
    (label_mic_active, set_label_mic_active, "Mic active", "Microfone ativo"),
    (label_muted_caps, set_label_muted_caps, "MUTED", "MUDO"),
    (update_prompt, set_update_prompt, "A new version of Litecord with improvements and fixes is ready to install. Update now?", "Uma nova versão do Litecord com melhorias e correções está pronta para instalação. Deseja atualizar agora?"),
    (screen_window, set_screen_window, "Screen / Window", "Tela / Janela"),
    (system_portal, set_system_portal, "Native system portal", "Portal nativo do sistema"),
    (app_music, set_app_music, "Music", "Música"),
    (app_moderation, set_app_moderation, "Moderation", "Moderação"),
    (app_utility, set_app_utility, "Utility", "Utilidade"),
    (cmd_play_desc, set_cmd_play_desc, "Play music, audio or a playlist (YouTube/Spotify/SoundCloud)", "Tocar música, áudio ou playlist (YouTube/Spotify/SoundCloud)"),
    (cmd_play_usage, set_cmd_play_usage, "/play <name or link>", "/play <nome ou link>"),
    (cmd_play_param, set_cmd_play_param, "Song name or YouTube/Spotify/SoundCloud link", "Nome da música ou link do YouTube/Spotify/SoundCloud"),
    (cmd_skip_desc, set_cmd_skip_desc, "Skip to the next track in the queue", "Pular para a próxima faixa da fila"),
    (cmd_stop_desc, set_cmd_stop_desc, "Stop the bot's playback and clear the queue", "Parar reprodução do bot e limpar a fila"),
    (cmd_pause_desc, set_cmd_pause_desc, "Pause the song that is playing", "Pausar a música em reprodução"),
    (cmd_resume_desc, set_cmd_resume_desc, "Resume the paused playback", "Retomar a reprodução pausada"),
    (cmd_queue_desc, set_cmd_queue_desc, "Show the list of songs in the queue", "Visualizar a lista de músicas na fila"),
    (cmd_nowplaying_desc, set_cmd_nowplaying_desc, "Show details of the song playing right now", "Exibir detalhes da música que está tocando agora"),
    (cmd_volume_desc, set_cmd_volume_desc, "Adjust the bot's playback volume", "Ajustar o volume de reprodução do bot"),
    (cmd_volume_param, set_cmd_volume_param, "Volume from 1 to 100", "Volume de 1 a 100"),
    (cmd_loop_desc, set_cmd_loop_desc, "Enable or disable repeat for the song or queue", "Ativar ou desativar repetição da música ou fila"),
    (cmd_loop_param, set_cmd_loop_param, "Mode: track, queue or off", "Modo: track, queue ou off"),
    (cmd_shuffle_desc, set_cmd_shuffle_desc, "Shuffle the song queue", "Embaralhar a fila de músicas"),
    (cmd_lyrics_desc, set_cmd_lyrics_desc, "Search for and show the lyrics of the current song", "Buscar e exibir a letra da música atual"),
    (cmd_lyrics_param, set_cmd_lyrics_param, "Song name (optional)", "Nome da música (opcional)"),
    (cmd_247_desc, set_cmd_247_desc, "Keep the bot connected 24/7 in the voice channel", "Manter o bot 24/7 conectado no canal de voz"),
    (cmd_help_desc, set_cmd_help_desc, "Show the bot's help menu and commands", "Exibir o menu de ajuda e comandos do bot"),
    (cmd_ping_desc, set_cmd_ping_desc, "Check the latency and response time", "Verificar a latência e tempo de resposta"),
    (cmd_clear_desc, set_cmd_clear_desc, "Clear recent messages in the channel", "Limpar mensagens recentes do canal"),
    (cmd_clear_usage, set_cmd_clear_usage, "/clear <amount>", "/clear <quantidade>"),
    (cmd_clear_param, set_cmd_clear_param, "Number of messages to delete", "Quantidade de mensagens a apagar"),
    (cmd_avatar_desc, set_cmd_avatar_desc, "Show a user's enlarged avatar", "Exibir o avatar ampliado de um usuário"),
    (cmd_avatar_usage, set_cmd_avatar_usage, "/avatar [@user]", "/avatar [@usuario]"),
    (cmd_user_param, set_cmd_user_param, "Discord user", "Usuário do Discord"),
    (cmd_userinfo_desc, set_cmd_userinfo_desc, "View detailed information about a user's account", "Ver informações detalhadas da conta de um usuário"),
    (cmd_userinfo_usage, set_cmd_userinfo_usage, "/userinfo [@user]", "/userinfo [@usuario]"),
    (cmd_serverinfo_desc, set_cmd_serverinfo_desc, "View statistics and information about the current server", "Ver estatísticas e informações do servidor atual"),
    (mic_state_active, set_mic_state_active, "ACTIVE", "ATIVO"),
    (mic_state_silent, set_mic_state_silent, "SILENCE", "SILÊNCIO"),
    (keybind_none, set_keybind_none, "None / Disabled", "Nenhum / Desativado"),
    (recording, set_recording, "Recording...", "Gravando..."),
    (testing, set_testing, "Testing...", "Testando..."),
    (test_proxy, set_test_proxy, "Test Proxy", "Testar Proxy"),
    (checking, set_checking, "Checking...", "Verificando..."),
    (check_updates, set_check_updates, "Check for Updates", "Verificar Atualizações"),
    (loading_older, set_loading_older, "Loading older messages...", "Carregando mensagens anteriores..."),
    (load_older, set_load_older, "Load older messages", "Carregar mensagens mais antigas"),
    (downloading, set_downloading, "Downloading...", "Baixando..."),
    (download_image_size, set_download_image_size, "Download Image (", "Baixar Imagem ("),
    (download_image, set_download_image, "Download Image", "Baixar Imagem"),
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Remembers the (already resolved) language so Rust-side messages follow the UI.
pub fn set_current(lang: Language) {
    let v = if lang == Language::Portuguese { 1 } else { 0 };
    CURRENT.store(v, Ordering::Relaxed);
}

/// Strings for the language currently shown in the UI.
pub fn ui_text() -> &'static UiStrings {
    table_for(if CURRENT.load(Ordering::Relaxed) == 1 { Language::Portuguese } else { Language::English })
}

pub fn table_for(lang: Language) -> &'static UiStrings {
    match lang {
        Language::Portuguese => &PT_UI,
        _ => &EN_UI,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_string_exists_in_both_languages() {
        for (key, en, pt) in entries() {
            assert!(!en.trim().is_empty(), "{key}: empty English text");
            assert!(!pt.trim().is_empty(), "{key}: empty Portuguese text");
        }
    }

    #[test]
    fn english_table_has_no_portuguese_leftovers() {
        let accents = "áàâãéêíóôõúçÁÀÂÃÉÊÍÓÔÕÚÇ";
        for (key, en, _) in entries() {
            assert!(
                !en.chars().any(|c| accents.contains(c)),
                "{key}: English text still looks Portuguese: {en}"
            );
        }
    }

    #[test]
    fn placeholders_are_kept_in_every_language() {
        assert!(EN_UI.proxy_failed.contains("{}"));
        assert!(PT_UI.proxy_failed.contains("{}"));
    }

    #[test]
    fn current_language_selects_the_matching_table() {
        set_current(Language::Portuguese);
        assert_eq!(ui_text().not_connected, PT_UI.not_connected);
        set_current(Language::English);
        assert_eq!(ui_text().not_connected, EN_UI.not_connected);
    }
}
