# ClawZ Design System

## Overview

The ClawZ Design System defines the visual language for the ClawZ Agent Orchestration Platform's Tauri desktop and mobile interfaces. It is built around the brand's copper and silver metallic identity, providing both light and dark themes that adapt automatically to the user's OS preference.

---

## Brand Identity

### Logos

| Asset | File | Usage |
|-------|------|-------|
| Copper Full Logo | `docs/Logo/clawz-copper-f.png` | Light theme — headers, splash screens |
| Copper Icon | `docs/Logo/clawz-copper.png` | Light theme — favicons, compact spaces |
| Silver Full Logo | `docs/Logo/clawz-silver-f.png` | Dark theme — headers, splash screens |
| Silver Icon | `docs/Logo/clawz-silver.png` | Dark theme — favicons, compact spaces |

**Rule:** Silver variants are used on dark backgrounds; copper variants are used on light backgrounds.

### Brand Colors

| Name | Hex | Usage |
|------|-----|-------|
| Copper | `#B87333` | Primary accent (light theme) |
| Copper Light | `#D4A574` | Gradients, highlights (light) |
| Copper Dark | `#8B5A2B` | Hover states, shadows (light) |
| Silver | `#C0C0C0` | Primary accent (dark theme) |
| Silver Light | `#E8E8E8` | Gradients, highlights (dark) |
| Silver Dark | `#888888` | Hover states, shadows (dark) |

---

## Typography

### Font Stack

| Role | Font | Weights | Usage |
|------|------|---------|-------|
| Headings | Inter | 600, 700, 800 | Page titles, section headers |
| Body | Inter | 400, 500 | UI text, paragraphs, labels |
| Mono | JetBrains Mono | 400, 500 | Code snippets, agent IDs, data |

### Type Scale

| Token | Size | Line Height | Weight | Usage |
|-------|------|-------------|--------|-------|
| H1 | 40px | 1.2 | 800 | Hero titles |
| H2 | 32px | 1.25 | 700 | Section titles |
| H3 | 24px | 1.3 | 600 | Card titles, panel headers |
| Body | 16px | 1.6 | 400 | Paragraphs, descriptions |
| Body Small | 14px | 1.5 | 400 | Captions, metadata |
| Label | 12px | 1.4 | 600 | Tags, status badges |
| Mono | 14px | 1.5 | 400 | Code, IDs, CLI output |

---

## Color Tokens

### Light Theme (Copper)

| Token | Hex | Usage |
|-------|-----|-------|
| `--bg-page` | `#FAF7F4` | App background |
| `--bg-surface` | `#FFFFFF` | Cards, panels |
| `--bg-elevated` | `#FFFFFF` | Inputs, modals |
| `--fg-primary` | `#1A1410` | Primary text |
| `--fg-secondary` | `#5C4D42` | Secondary text |
| `--fg-tertiary` | `#8B7D72` | Muted text, placeholders |
| `--border-subtle` | `#E8DDD4` | Dividers, separators |
| `--border-default` | `#D4C8BC` | Card borders, input borders |
| `--border-strong` | `#B8A99C` | Focus rings, active borders |
| `--accent-primary` | `#B87333` | Primary buttons, links |
| `--accent-secondary` | `#787878` | Secondary actions |
| `--accent-success` | `#2D8A4E` | Online, healthy |
| `--accent-warning` | `#C78B2A` | Busy, warning |
| `--accent-danger` | `#C23B3B` | Error, offline |
| `--accent-info` | `#4A7BAF` | Info, idle |

### Dark Theme (Silver)

| Token | Hex | Usage |
|-------|-----|-------|
| `--bg-page` | `#0D0F12` | App background |
| `--bg-surface` | `#161A20` | Cards, panels |
| `--bg-elevated` | `#1E2329` | Inputs, modals |
| `--fg-primary` | `#E8E8E8` | Primary text |
| `--fg-secondary` | `#A8ACB2` | Secondary text |
| `--fg-tertiary` | `#6E747C` | Muted text, placeholders |
| `--border-subtle` | `#2A3038` | Dividers, separators |
| `--border-default` | `#3A424C` | Card borders, input borders |
| `--border-strong` | `#4E5660` | Focus rings, active borders |
| `--accent-primary` | `#C0C0C0` | Primary buttons, links |
| `--accent-secondary` | `#787878` | Secondary actions |
| `--accent-success` | `#4ADE80` | Online, healthy |
| `--accent-warning` | `#FBBF24` | Busy, warning |
| `--accent-danger` | `#F87171` | Error, offline |
| `--accent-info` | `#60A5FA` | Info, idle |

---

## Spacing System

Based on 4px increments:

| Token | Value |
|-------|-------|
| `--spacing-xs` | 4px |
| `--spacing-sm` | 8px |
| `--spacing-md` | 16px |
| `--spacing-lg` | 24px |
| `--spacing-xl` | 32px |
| `--spacing-2xl` | 48px |
| `--spacing-3xl` | 64px |

---

## Shadows

| Token | Value |
|-------|-------|
| `--shadow-sm` | `0 1px 2px rgba(26, 20, 16, 0.04)` |
| `--shadow-md` | `0 4px 12px rgba(26, 20, 16, 0.08)` |
| `--shadow-lg` | `0 12px 32px rgba(26, 20, 16, 0.12)` |
| `--shadow-glow` | `0 0 24px rgba(184, 115, 51, 0.15)` |

Dark theme shadows use `rgba(0, 0, 0, 0.3–0.5)` instead.

---

## Border Radius

| Token | Value |
|-------|-------|
| `--radius-sm` | 6px |
| `--radius-md` | 10px |
| `--radius-lg` | 16px |
| `--radius-xl` | 24px |

---

## Animation Tokens

| Token | Value |
|-------|-------|
| `--transition-fast` | `150ms ease` |
| `--transition-base` | `250ms ease` |

---

## Component Specs

### Buttons

| Variant | Background | Text | Border | Shadow |
|---------|------------|------|--------|--------|
| Primary | Gradient (Copper → Copper Dark) | `#FFFFFF` | None | Glow |
| Secondary | `--bg-elevated` | `--fg-primary` | `--border-default` | None |
| Ghost | Transparent | `--fg-secondary` | None | None |

All buttons: `padding: 12px 24px`, `border-radius: 10px`, `font-weight: 600`.

### Cards

- `background: --bg-surface`
- `border: 1px solid --border-subtle`
- `border-radius: 16px`
- `padding: 24px`
- Hover: `box-shadow: --shadow-md`, `border-color: --border-default`

### Inputs

- `background: --bg-elevated`
- `border: 1px solid --border-default`
- `border-radius: 10px`
- `padding: 10px 14px`
- Focus: `border-color: --accent-primary`, `box-shadow: 0 0 0 3px rgba(184,115,51,0.15)`

---

## Responsive Breakpoints

| Name | Width | Usage |
|------|-------|-------|
| Mobile | < 768px | Single column, bottom nav |
| Tablet | 768–1023px | 2-column grids, sidebar collapses |
| Desktop | ≥ 1024px | Full sidebar, multi-column layouts |

---

## Accessibility Requirements

1. **Contrast:** All text meets WCAG AA (4.5:1). Primary text meets AAA (7:1) where possible.
2. **Focus:** Visible focus rings (`2px solid --accent-primary`, `3px` offset).
3. **Motion:** Respect `prefers-reduced-motion`. All animations are < 300ms.
4. **Touch:** Minimum tap target 44×44px.
5. **Screen readers:** All interactive elements have accessible labels.

---

## File Structure

```
crates/clawz-tauri/
├── design/
│   ├── design-system.md        # This document
│   └── design-preview.html     # Interactive preview
├── src-ui/                     # Frontend source (HTML/CSS/JS)
│   ├── index.html
│   ├── styles/
│   │   ├── tokens.css          # CSS custom properties
│   │   ├── base.css
│   │   └── components.css
│   ├── components/
│   │   ├── Button.js
│   │   ├── Card.js
│   │   └── Sidebar.js
│   └── app.js
├── src/                        # Rust Tauri backend
│   ├── lib.rs
│   ├── main.rs
│   ├── commands.rs
│   └── tray.rs
├── Cargo.toml
├── build.rs
└── tauri.conf.json
```

---

## Implementation Notes

1. **Theme switching:** Detect `prefers-color-scheme` via JavaScript/CSS. Tauri provides OS theme events.
2. **Logo swapping:** Toggle between copper and silver logo assets when the theme changes.
3. **Tauri CSP:** Ensure `tauri.conf.json` CSP allows inline styles for the theme system.
4. **Font loading:** Bundle Inter and JetBrains Mono or load from Google Fonts with local fallback.
5. **Mobile frame:** For mobile Tauri builds, use the 320px-wide mobile mockup layout with a bottom navigation bar replacing the sidebar.
