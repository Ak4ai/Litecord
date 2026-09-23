# 🎨 User Interface & Design System

Litecord’s graphical interface is built with **Slint 1.9**, compiled ahead-of-time directly into native machine code.

---

## 1. Reactive UI Model

Unlike web-based Electron clients that manipulate a DOM tree, Slint renders via hardware-accelerated backends (Direct3D 11, Vulkan, or Software Renderer).
- Properties are declarative: UI elements automatically re-render only when their bound properties change.
- No garbage collection pauses: Slint allocations are handled in deterministic native memory.

---

## 2. Color Palette & Discord Theme

| Token | Hex | Usage |
| :--- | :--- | :--- |
| **Background Dark** | `#1e1f22` | Server bar, footer, stage backgrounds |
| **Background Secondary** | `#2b2d31` | Channel lists, sidebars |
| **Background Primary** | `#313338` | Chat canvas, main content |
| **Accent Blurple** | `#5865f2` | Primary buttons, active channels |
| **Brand Blue** | `#2563eb` | Settings active tabs, HUD indicators |
| **Success Green** | `#23a55a` | Online status, VAD active, volume levels |
| **Warning Yellow** | `#f0b232` | Warnings, threshold markers |
| **Danger Red** | `#f23f43` | Muted states, disconnect, critical alerts |
| **Text Normal** | `#dbdee1` | Main message bodies |
| **Text Muted** | `#949ba4` | Subtitles, timestamps, guides |

---

## 3. Unified Emojis & Vector SVG Icons

- All UI action buttons use clean vector SVG icons (`assets/*.svg`), guaranteeing crisp rendering without blurry pixels or missing glyph boxes (`□`).
  - `dm-chat.svg`: Discord-style chat bubble for the direct messages entry point.
  - `phone.svg`: Start call / answer incoming voice call.
  - `phone-slash.svg`: Hang up / reject incoming voice call.
- Discord custom emojis and Unicode Twemojis are indexed and rendered directly in chat messages and embeds.

---

## 4. Direct Messages (DMs) & Unread Badges

- **DM Navigation**: Instant access via the top chat bubble button on the server sidebar.
- **Unread Badge Alignment**: Designed with geometric symmetry matching voice user counters, including a 1px sub-pixel optical offset for visual centering across fonts.
- **Incoming Call Card**: Floating modal presenting avatar, username, and green/red interactive action buttons.
- **Event Deduplication**: Gateway `MESSAGE_CREATE` events are deduplicated at the controller level to prevent badge inflation from redundant gateway frames.

---

## 5. Light & Dark Themes (AppTheme)

- **Native Light Theme**: Optional light interface designed for reading clarity in daylight environments, contributed by `@starzynhobr` (PR #23).
- **Semantic Theme Tokens**: Centralized in `AppTheme` within Slint, decoupling color definitions from UI component layout.
- **Persistent Preference**: Theme toggle in the Settings modal persists across restarts in `.litecord_user_settings.json`.

---

## 6. Voice Channel Participants & Permission Filtering

- **Participants Preview**: Displays members currently in voice channels directly on the sidebar (contributed by `@starzynhobr` in PR #22), allowing users to see who is speaking before joining.
- **Direct Connect**: Clicking a participant automatically switches audio to that voice channel.
- **Permission Filtering (`VIEW_CHANNEL`)**: Automatically hides inaccessible channels and empty categories based on Discord permission hierarchy.
