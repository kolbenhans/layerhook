# layerhook

Switches your QMK/Vial keyboard's active layer automatically based on which
window is focused — regex rules map a window title to a layer, with a
fallback default layer for everything else.

<img width="500" height="500" alt="code-snippet" src="https://github.com/user-attachments/assets/40a8919a-3197-417e-acb5-ec6eee84bb3d" />

## How it works

A background thread watches for window-focus changes (event-driven — pushed
by the OS/compositor the instant focus changes, not polled), matches the new
title against your rules (first match wins), and sends the target layer to
the keyboard over raw HID. No match → falls back to the configured default
layer. A periodic recheck every 500ms still runs alongside this purely as a
retry (e.g. the keyboard was briefly unplugged) — it doesn't drive normal
layer switching, which reacts immediately.

- **What a rule matches against:** on Linux, the window title. On Windows, `[exe.exe]:window title` (same format as OBS's window picker), e.g. `[Photoshop.exe]:Untitled-1 @ 50% (RGB/8)` — so `Photoshop\.exe` covers every window of the app, even panels with no title of their own. A title that can't be read shows as `(null)`, an unreadable exe as `(unknown)`.
- Rules are checked top to bottom, first match wins — drag the `☰` handle to reorder.
- Optional "Always on top" (Windows and X11 only — Wayland has no protocol for it; use your compositor's pin/keep-above function there).
- Runs in the system tray; closing the window hides it instead of quitting.
- Optional autostart on login (Linux: XDG autostart entry; Windows: `HKCU...\Run`).

## Supported OS / desktop

| OS / desktop | Mechanism | Status |
| --- | --- | --- |
| Windows | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` | ✅ Tested |
| Hyprland | `wlr-foreign-toplevel-management-unstable-v1` | ✅ Tested |
| KDE Plasma (Wayland) | `plasma-window-management` (KWin) | ✅ Tested |
| COSMIC | `cosmic-toplevel-info-unstable-v1` + `ext-foreign-toplevel-list-v1` | ✅ Tested |
| Sway, River | `wlr-foreign-toplevel-management-unstable-v1` (same as Hyprland) | ⚠️ Should work, not tested |
| X11 — i3, XFCE, MATE, KDE/GNOME in an X11 session, etc. | `_NET_ACTIVE_WINDOW`/EWMH | ⚠️ Should work, not tested |
| GNOME (Wayland/Mutter) | — | ❌ Not supported |

**Why not GNOME:** Mutter deliberately doesn't implement
`wlr-foreign-toplevel-management` (that's a wlroots-ecosystem protocol) or
any other standard way for a client to ask "what's focused" — it's treated as
a sandboxing/privacy boundary, by design, not an oversight. The only way in
is a GNOME Shell extension (e.g. "Window Calls") exposing it over D-Bus,
which the user would have to install separately — not wired up here.

## Firmware requirement

This is **not** a generic VIA/Vial tool — it needs two custom raw HID
commands added to the keyboard's own firmware (family `0x02`):

- `0xB0` **SET_LAYER**: `[0x02, 0xB0, layer]` → `layer_move(layer)`, replies with `[0x02, 0xB0, layer]` once applied.
- `0xB1` **GET_LAYER**: `[0x02, 0xB1]` → replies `[0x02, 0xB1, current_layer]`.

Easiest path: the [`layerhook` QMK community module](https://github.com/kolbenhans/qmk-modules#layerhook)
implements both commands standalone — add it to your keymap's `keymap.json`
and you're done, no hand-written `case` needed. It also chains automatically
if the keymap also uses the `key_colors`/`audio_visualizer` modules from the
same repo.

Reference implementation without the module (BCORNE, `keyColors` keymap):
[`key_colors_hid.c`](https://github.com/kolbenhans/BCORNE/blob/main/m57_bcorne/keymaps/keyColors/key_colors_hid.c#L155-L179)
— add an equivalent `case` to your own keyboard's `raw_hid_receive_kb` to use
layerhook with it.

## Building

```sh
cargo build --release --bin layerhook
```

Cross-compile for Windows from Linux:

```sh
rustup target add x86_64-pc-windows-gnu
cargo build --release --bin layerhook --target x86_64-pc-windows-gnu
```

Two extra diagnostic binaries exist for testing the firmware side directly,
independent of the GUI:

- `cargo run --bin probe` — one-shot SET_LAYER/GET_LAYER round-trip check.
- `cargo run --bin cycle_layers` — cycles layers 1-4 (3s each) to eyeball a
  per-layer RGB table.

## Status

Early beta. Built for one specific keyboard (BCORNE). Window detection has
been tested end-to-end on the desktops marked ✅ above; the rest of the
architecture (rules, HID, tray, autostart) is shared and desktop-independent.
