# layerhook

Switches your QMK/Vial keyboard's active layer automatically based on which
window is focused — regex rules map a window title to a layer, with a
fallback default layer for everything else.

<img width="500" height="500" alt="code-snippet" src="https://github.com/user-attachments/assets/40a8919a-3197-417e-acb5-ec6eee84bb3d" />

## How it works

A background thread polls the focused window's title every 500ms, matches it
against your rules (first match wins), and sends the target layer to the
keyboard over raw HID. No match → falls back to the configured default layer.

- **Linux**: window detection via `hyprctl` — **Hyprland only** for now.
- **Windows**: window detection via Win32 (`GetForegroundWindow`/`EnumWindows`) — works on any window manager.
- Runs in the system tray; closing the window hides it instead of quitting.
- Optional autostart on login (Linux: XDG autostart entry; Windows: `HKCU...\Run`).

## Firmware requirement

This is **not** a generic VIA/Vial tool — it needs two custom raw HID
commands added to the keyboard's own firmware (family `0x02`):

- `0xB0` **SET_LAYER**: `[0x02, 0xB0, layer]` → `layer_move(layer)`, replies with `[0x02, 0xB0, layer]` once applied.
- `0xB1` **GET_LAYER**: `[0x02, 0xB1]` → replies `[0x02, 0xB1, current_layer]`.

Reference implementation (BCORNE, `keyColors` keymap):
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

Early beta. Built for one specific keyboard (BCORNE) and one specific
compositor (Hyprland) — the architecture is generic, but only that
combination has been tested end-to-end so far.
