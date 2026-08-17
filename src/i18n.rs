use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Auto,
    English,
    Portuguese,
    Spanish,
    German,
    French,
    Russian,
    Japanese,
}

impl Language {
    pub fn code(&self) -> &'static str {
        match self {
            Language::Auto => "auto",
            Language::English => "en",
            Language::Portuguese => "pt",
            Language::Spanish => "es",
            Language::German => "de",
            Language::French => "fr",
            Language::Russian => "ru",
            Language::Japanese => "ja",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code.to_lowercase().as_str() {
            "en" | "english" => Language::English,
            "pt" | "pt-br" | "pt-pt" | "portuguese" => Language::Portuguese,
            "es" | "spanish" | "espanol" => Language::Spanish,
            "de" | "german" | "deutsch" => Language::German,
            "fr" | "french" | "francais" => Language::French,
            "ru" | "russian" => Language::Russian,
            "ja" | "japanese" => Language::Japanese,
            _ => Language::Auto,
        }
    }

    pub fn display_info(&self) -> (&'static str, &'static str) {
        match self {
            Language::Auto => ("🌐 Auto (System)", "Detect OS Language"),
            Language::English => ("🇺🇸 English (US)", "Default Global"),
            Language::Portuguese => ("🇧🇷 Português (Brasil)", "Portuguese"),
            Language::Spanish => ("🇪🇸 Español", "Spanish"),
            Language::German => ("🇩🇪 Deutsch", "German"),
            Language::French => ("🇫🇷 Français", "French"),
            Language::Russian => ("🇷🇺 Русский", "Russian"),
            Language::Japanese => ("🇯🇵 日本語", "Japanese"),
        }
    }

    pub fn all_available() -> &'static [Language] {
        &[
            Language::Auto,
            Language::English,
            Language::Portuguese,
            Language::Spanish,
            Language::German,
            Language::French,
            Language::Russian,
            Language::Japanese,
        ]
    }
}

pub struct Translations {
    pub login_title: &'static str,
    pub login_desc: &'static str,
    pub login_placeholder: &'static str,
    pub login_btn_connect: &'static str,
    pub login_btn_detect: &'static str,
    pub server_channels: &'static str,
    pub leave: &'static str,
    pub view_text_chat: &'static str,
    pub leave_call: &'static str,
    pub voice_participants_title: &'static str,
    pub you: &'static str,
    pub view_voice_room: &'static str,
    pub replying_to: &'static str,
    pub chat_placeholder_prefix: &'static str,
    pub send: &'static str,
    pub settings_title: &'static str,
    pub settings_language_label: &'static str,
    pub settings_input_device: &'static str,
    pub settings_output_device: &'static str,
    pub settings_threshold: &'static str,
    pub settings_mic_level: &'static str,
    pub settings_done: &'static str,
    pub logout_title: &'static str,
    pub logout_confirm_prefix: &'static str,
    pub cancel: &'static str,
    pub confirm_logout: &'static str,
    pub voice_connecting: &'static str,
    pub voice_connecting_title: &'static str,
    pub voice_connecting_desc: &'static str,
    pub voice_connected: &'static str,
}

pub const EN_TRANSLATIONS: Translations = Translations {
    login_title: "Sign in to Litecord",
    login_desc: "Enter your Discord User Token for direct connection to Gateway v9.\n(Estimated RAM usage ~15MB)",
    login_placeholder: "Paste your Discord User Token here...",
    login_btn_connect: "Connect to Gateway",
    login_btn_detect: "Detect Discord Token",
    server_channels: "SERVER CHANNELS",
    leave: "Leave",
    view_text_chat: "View Text Chat",
    leave_call: "Disconnect Call",
    voice_participants_title: "VOICE ROOM PARTICIPANTS",
    you: "YOU",
    view_voice_room: "View Voice Room",
    replying_to: "Replying to @",
    chat_placeholder_prefix: "Message ",
    send: "Send",
    settings_title: "Voice & Audio Settings",
    settings_language_label: "LANGUAGE / IDIOMA",
    settings_input_device: "INPUT DEVICE (MICROPHONE)",
    settings_output_device: "OUTPUT DEVICE (SPEAKERS / HEADPHONES)",
    settings_threshold: "VOICE DETECTION SENSITIVITY (THRESHOLD)",
    settings_mic_level: "REAL-TIME MICROPHONE LEVEL",
    settings_done: "Done",
    logout_title: "Sign Out",
    logout_confirm_prefix: "Do you want to log out from ",
    cancel: "Cancel",
    confirm_logout: "Sign Out",
    voice_connecting: "Connecting...",
    voice_connecting_title: "Connecting to Voice Channel...",
    voice_connecting_desc: "Authenticating with Voice Gateway v9 and negotiating DAVE E2EE encryption...",
    voice_connected: "Voice Connected",
};

pub const PT_TRANSLATIONS: Translations = Translations {
    login_title: "Entrar no Litecord",
    login_desc: "Insira seu Discord User Token para conexão direta à Gateway v9.\n(Consumo de RAM estimado em ~15MB)",
    login_placeholder: "Cole seu Discord User Token aqui...",
    login_btn_connect: "Conectar à Gateway",
    login_btn_detect: "Detectar Token do Discord",
    server_channels: "CANAIS DO SERVIDOR",
    leave: "Sair",
    view_text_chat: "Ver Chat de Texto",
    leave_call: "Sair da Call",
    voice_participants_title: "PARTICIPANTES NA SALA DE ÁUDIO",
    you: "VOCÊ",
    view_voice_room: "Ver Sala de Voz",
    replying_to: "Respondendo a @",
    chat_placeholder_prefix: "Conversar em ",
    send: "Enviar",
    settings_title: "Configurações de Voz e Áudio",
    settings_language_label: "IDIOMA / LANGUAGE",
    settings_input_device: "DISPOSITIVO DE ENTRADA (MICROFONE)",
    settings_output_device: "DISPOSITIVO DE SAÍDA (ALTO-FALANTES / FONE DE OUVIDO)",
    settings_threshold: "SENSIBILIDADE DE DETECÇÃO DE VOZ (THRESHOLD)",
    settings_mic_level: "NÍVEL DO MICROFONE EM TEMPO REAL",
    settings_done: "Concluído",
    logout_title: "Sair da Conta",
    logout_confirm_prefix: "Deseja desconectar de ",
    cancel: "Voltar",
    confirm_logout: "Sair",
    voice_connecting: "Conectando...",
    voice_connecting_title: "Conectando à Sala de Voz...",
    voice_connecting_desc: "Autenticando na Voice Gateway v9 e negociando criptografia ponta-a-ponta (DAVE)...",
    voice_connected: "Voz Conectada",
};

pub const ES_TRANSLATIONS: Translations = Translations {
    login_title: "Iniciar sesión en Litecord",
    login_desc: "Introduce tu Discord User Token para conexión directa a Gateway v9.\n(Consumo de RAM estimado en ~15MB)",
    login_placeholder: "Pega tu Discord User Token aquí...",
    login_btn_connect: "Conectar a la Gateway",
    login_btn_detect: "Detectar Token de Discord",
    server_channels: "CANALES DEL SERVIDOR",
    leave: "Salir",
    view_text_chat: "Ver Chat de Texto",
    leave_call: "Desconectar Llamada",
    voice_participants_title: "PARTICIPANTES EN LA SALA DE VOZ",
    you: "TÚ",
    view_voice_room: "Ver Sala de Voz",
    replying_to: "Respondiendo a @",
    chat_placeholder_prefix: "Enviar mensaje a ",
    send: "Enviar",
    settings_title: "Ajustes de Voz y Audio",
    settings_language_label: "IDIOMA / LANGUAGE",
    settings_input_device: "DISPOSITIVO DE ENTRADA (MICRÓFONO)",
    settings_output_device: "DISPOSITIVO DE SALIDA (ALTAVOCES / AURICULARES)",
    settings_threshold: "SENSIBILIDAD DE DETECCIÓN DE VOZ (THRESHOLD)",
    settings_mic_level: "NIVEL DE MICRÓFONO EN TIEMPO REAL",
    settings_done: "Hecho",
    logout_title: "Cerrar Sesión",
    logout_confirm_prefix: "¿Deseas desconectarte de ",
    cancel: "Cancelar",
    confirm_logout: "Cerrar Sesión",
    voice_connecting: "Conectando...",
    voice_connecting_title: "Conectando a la Sala de Voz...",
    voice_connecting_desc: "Autenticando en Voice Gateway v9 y negociando cifrado E2EE (DAVE)...",
    voice_connected: "Voz Conectada",
};

pub const DE_TRANSLATIONS: Translations = Translations {
    login_title: "Bei Litecord anmelden",
    login_desc: "Gib deinen Discord User Token für direkte Gateway v9 Verbindung ein.\n(Geschätzter RAM-Verbrauch ~15MB)",
    login_placeholder: "Discord User Token hier einfügen...",
    login_btn_connect: "Mit Gateway verbinden",
    login_btn_detect: "Discord Token erkennen",
    server_channels: "SERVERKANÄLE",
    leave: "Verlassen",
    view_text_chat: "Textchat anzeigen",
    leave_call: "Anruf beenden",
    voice_participants_title: "TEILNEHMER IM SPRACHRAUM",
    you: "DU",
    view_voice_room: "Sprachraum anzeigen",
    replying_to: "Antwort auf @",
    chat_placeholder_prefix: "Nachricht an ",
    send: "Senden",
    settings_title: "Sprach- und Audioeinstellungen",
    settings_language_label: "SPRACHE / LANGUAGE",
    settings_input_device: "EINGABEGERÄT (MIKROFON)",
    settings_output_device: "AUSGABEGERÄT (LAUTSPRECHER / KOPFHÖRER)",
    settings_threshold: "SPRACHAKTIVIERUNG-EMPFINDLICHKEIT (THRESHOLD)",
    settings_mic_level: "ECHTZEIT-MIKROFONPEGEL",
    settings_done: "Fertig",
    logout_title: "Abmelden",
    logout_confirm_prefix: "Möchtest du dich abmelden von ",
    cancel: "Abbrechen",
    confirm_logout: "Abmelden",
    voice_connecting: "Verbinden...",
    voice_connecting_title: "Verbindung zum Sprachkanal...",
    voice_connecting_desc: "Authentifizierung mit Voice Gateway v9 und Aushandlung der DAVE E2EE-Verschlüsselung...",
    voice_connected: "Sprache verbunden",
};

pub const FR_TRANSLATIONS: Translations = Translations {
    login_title: "Connexion à Litecord",
    login_desc: "Entrez votre Discord User Token pour connexion directe à Gateway v9.\n(RAM estimée ~15Mo)",
    login_placeholder: "Collez votre token Discord ici...",
    login_btn_connect: "Se connecter à la Gateway",
    login_btn_detect: "Détecter le Token Discord",
    server_channels: "SALONS DU SERVEUR",
    leave: "Quitter",
    view_text_chat: "Voir le Salon Textuel",
    leave_call: "Déconnecter l'Appel",
    voice_participants_title: "PARTICIPANTS DANS LE SALON VOCAL",
    you: "VOUS",
    view_voice_room: "Voir le Salon Vocal",
    replying_to: "En réponse à @",
    chat_placeholder_prefix: "Envoyer un message dans ",
    send: "Envoyer",
    settings_title: "Paramètres Vocaux et Audio",
    settings_language_label: "LANGUE / LANGUAGE",
    settings_input_device: "PÉRIPHÉRIQUE D'ENTRÉE (MICROPHONE)",
    settings_output_device: "PÉRIPHÉRIQUE DE SORTIE (CASQUE / ENCEINTES)",
    settings_threshold: "SENSIBILITÉ DE DÉTECTION VOCALE (THRESHOLD)",
    settings_mic_level: "NIVEAU DU MICROPHONE EN TEMPS RÉEL",
    settings_done: "Terminé",
    logout_title: "Se Déconnecter",
    logout_confirm_prefix: "Voulez-vous vous déconnecter de ",
    cancel: "Annuler",
    confirm_logout: "Déconnexion",
    voice_connecting: "Connexion...",
    voice_connecting_title: "Connexion au Salon Vocal...",
    voice_connecting_desc: "Authentification auprès de la Voice Gateway v9 et négociation du chiffrement DAVE E2EE...",
    voice_connected: "Vocal Connecté",
};

pub const RU_TRANSLATIONS: Translations = Translations {
    login_title: "Вход в Litecord",
    login_desc: "Введите Discord User Token для подключения к Gateway v9.\n(Потребление ОЗУ ~15МБ)",
    login_placeholder: "Вставьте токен Discord сюда...",
    login_btn_connect: "Подключиться к Gateway",
    login_btn_detect: "Найти токен Discord",
    server_channels: "КАНАЛЫ СЕРВЕРА",
    leave: "Выйти",
    view_text_chat: "Текстовый чат",
    leave_call: "Покинуть звонок",
    voice_participants_title: "УЧАСТНИКИ В ГОЛОСОВОМ КАНАЛЕ",
    you: "ВЫ",
    view_voice_room: "Голосовой канал",
    replying_to: "В ответ @",
    chat_placeholder_prefix: "Написать в ",
    send: "Отправить",
    settings_title: "Настройки звука и голоса",
    settings_language_label: "ЯЗЫК / LANGUAGE",
    settings_input_device: "УСТРОЙСТВО ВВОДА (МИКРОФОН)",
    settings_output_device: "УСТРОЙСТВО ВЫВОДА (ДИНАМИКИ / НАУШНИКИ)",
    settings_threshold: "ЧУВСТВИТЕЛЬНОСТЬ ГОЛОСА (THRESHOLD)",
    settings_mic_level: "УРОВЕНЬ МИКРОФОНА В РЕАЛЬНОМ ВРЕМЕНИ",
    settings_done: "Готово",
    logout_title: "Выйти из аккаунта",
    logout_confirm_prefix: "Вы действительно хотите выйти из ",
    cancel: "Отмена",
    confirm_logout: "Выйти",
    voice_connecting: "Подключение...",
    voice_connecting_title: "Подключение к голосовому каналу...",
    voice_connecting_desc: "Аутентификация в Voice Gateway v9 и согласование E2EE шифрования (DAVE)...",
    voice_connected: "Голос подключен",
};

pub const JA_TRANSLATIONS: Translations = Translations {
    login_title: "Litecord にログイン",
    login_desc: "Gateway v9 に接続するために Discord ユーザートークンを入力してください。\n(推定RAM使用量: 約15MB)",
    login_placeholder: "Discord トークンをここに貼り付け...",
    login_btn_connect: "Gateway に接続",
    login_btn_detect: "Discord トークンを自動検出",
    server_channels: "サーバーチャンネル",
    leave: "退出",
    view_text_chat: "テキストチャットを表示",
    leave_call: "通話を終了",
    voice_participants_title: "ボイスルームの参加者",
    you: "あなた",
    view_voice_room: "ボイスルームを表示",
    replying_to: "返信先: @",
    chat_placeholder_prefix: "メッセージを送信: ",
    send: "送信",
    settings_title: "音声とオーディオ設定",
    settings_language_label: "言語 / LANGUAGE",
    settings_input_device: "入力デバイス (マイク)",
    settings_output_device: "出力デバイス (スピーカー / ヘッドホン)",
    settings_threshold: "音声検出感度 (THRESHOLD)",
    settings_mic_level: "リアルタイムマイク音量",
    settings_done: "完了",
    logout_title: "ログアウト",
    logout_confirm_prefix: "本当にログアウトしますか: ",
    cancel: "キャンセル",
    confirm_logout: "ログアウト",
    voice_connecting: "接続中...",
    voice_connecting_title: "ボイスチャンネルに接続中...",
    voice_connecting_desc: "Voice Gateway v9 に認証し、DAVE E2EE 暗号化を確立しています...",
    voice_connected: "音声接続完了",
};

const LANGUAGE_CONFIG_FILE: &str = ".litecord_language.json";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LanguageConfig {
    pub selected_language: String,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            selected_language: "auto".to_string(),
        }
    }
}

pub fn load_persisted_language_config() -> Language {
    if let Ok(data) = std::fs::read_to_string(LANGUAGE_CONFIG_FILE) {
        if let Ok(cfg) = serde_json::from_str::<LanguageConfig>(&data) {
            return Language::from_code(&cfg.selected_language);
        }
    }
    Language::Auto
}

pub fn save_persisted_language_config(lang: Language) {
    let cfg = LanguageConfig {
        selected_language: lang.code().to_string(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&cfg) {
        let _ = std::fs::write(LANGUAGE_CONFIG_FILE, json);
    }
}

#[cfg(target_os = "windows")]
pub fn detect_os_language() -> Language {
    if let Ok(lang) = std::env::var("LANG").or_else(|_| std::env::var("LC_ALL")) {
        let lower = lang.to_lowercase();
        if lower.starts_with("pt") { return Language::Portuguese; }
        if lower.starts_with("es") { return Language::Spanish; }
        if lower.starts_with("de") { return Language::German; }
        if lower.starts_with("fr") { return Language::French; }
        if lower.starts_with("ru") { return Language::Russian; }
        if lower.starts_with("ja") { return Language::Japanese; }
    }
    unsafe {
        let lang_id = windows_sys::Win32::Globalization::GetUserDefaultUILanguage();
        let primary_lang = lang_id & 0x03FF;
        match primary_lang {
            0x0016 => Language::Portuguese, // LANG_PORTUGUESE
            0x000A => Language::Spanish,    // LANG_SPANISH
            0x0007 => Language::German,     // LANG_GERMAN
            0x000C => Language::French,     // LANG_FRENCH
            0x0019 => Language::Russian,    // LANG_RUSSIAN
            0x0011 => Language::Japanese,   // LANG_JAPANESE
            _ => Language::English,
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn detect_os_language() -> Language {
    if let Ok(lang) = std::env::var("LANG").or_else(|_| std::env::var("LC_ALL")).or_else(|_| std::env::var("LC_MESSAGES")) {
        let lower = lang.to_lowercase();
        if lower.starts_with("pt") { return Language::Portuguese; }
        if lower.starts_with("es") { return Language::Spanish; }
        if lower.starts_with("de") { return Language::German; }
        if lower.starts_with("fr") { return Language::French; }
        if lower.starts_with("ru") { return Language::Russian; }
        if lower.starts_with("ja") { return Language::Japanese; }
    }
    Language::English
}
