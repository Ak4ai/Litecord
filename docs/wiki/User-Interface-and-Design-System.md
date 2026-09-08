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
- Discord custom emojis and Unicode Twemojis are indexed and rendered directly in chat messages and embeds.
