//! Native modifier key state.
//!
//! Port of `packages/tui/src/native-modifiers.ts` (66 LOC) together with the C
//! sources it loads as prebuilt addons: `native/darwin/src/darwin-modifiers.c`
//! and `native/win32/src/win32-console-mode.c`. Instead of a Node addon the
//! Rust port calls the OS APIs directly under `cfg(target_os)`
//! (deviation class 3, master plan: "native addons → direct OS calls").
//!
//! Pulled forward from task 13 of the workstream plan because
//! `ProcessTerminal::forward_input_sequence` needs it for the Shift+Enter
//! normalization.

/// Modifier keys the native helpers can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierKey {
    /// Shift.
    Shift,
    /// Command (macOS) / Windows key.
    Command,
    /// Control.
    Control,
    /// Option (macOS) / Alt.
    Option,
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ModifierKey;

    // CoreGraphics event flag masks (CGEventFlags), as used by
    // `native/darwin/src/darwin-modifiers.c`.
    const K_CG_EVENT_FLAG_MASK_SHIFT: u64 = 0x0002_0000;
    const K_CG_EVENT_FLAG_MASK_CONTROL: u64 = 0x0004_0000;
    const K_CG_EVENT_FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
    const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
    /// `kCGEventSourceStateCombinedSessionState`.
    const K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE: u32 = 0;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceFlagsState(state_id: u32) -> u64;
    }

    pub(super) fn is_modifier_pressed(key: ModifierKey) -> bool {
        let mask = match key {
            ModifierKey::Shift => K_CG_EVENT_FLAG_MASK_SHIFT,
            ModifierKey::Command => K_CG_EVENT_FLAG_MASK_COMMAND,
            ModifierKey::Control => K_CG_EVENT_FLAG_MASK_CONTROL,
            ModifierKey::Option => K_CG_EVENT_FLAG_MASK_ALTERNATE,
        };
        // SAFETY: CGEventSourceFlagsState only reads global input state.
        let flags =
            unsafe { CGEventSourceFlagsState(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE) };
        (flags & mask) != 0
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::ModifierKey;

    const KEY_PRESSED_MASK: u16 = 0x8000;
    const VK_SHIFT: i32 = 0x10;
    const VK_LSHIFT: i32 = 0xA0;
    const VK_RSHIFT: i32 = 0xA1;
    const VK_CONTROL: i32 = 0x11;
    const VK_LCONTROL: i32 = 0xA2;
    const VK_RCONTROL: i32 = 0xA3;
    const VK_MENU: i32 = 0x12;
    const VK_LMENU: i32 = 0xA4;
    const VK_RMENU: i32 = 0xA5;
    const VK_LWIN: i32 = 0x5B;
    const VK_RWIN: i32 = 0x5C;

    fn is_key_pressed(virtual_key: i32) -> bool {
        // SAFETY: GetAsyncKeyState only reads keyboard state.
        let state = unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(virtual_key)
        };
        (state as u16 & KEY_PRESSED_MASK) != 0
    }

    pub(super) fn is_modifier_pressed(key: ModifierKey) -> bool {
        match key {
            ModifierKey::Shift => {
                is_key_pressed(VK_SHIFT) || is_key_pressed(VK_LSHIFT) || is_key_pressed(VK_RSHIFT)
            }
            ModifierKey::Control => {
                is_key_pressed(VK_CONTROL)
                    || is_key_pressed(VK_LCONTROL)
                    || is_key_pressed(VK_RCONTROL)
            }
            ModifierKey::Option => {
                is_key_pressed(VK_MENU) || is_key_pressed(VK_LMENU) || is_key_pressed(VK_RMENU)
            }
            ModifierKey::Command => is_key_pressed(VK_LWIN) || is_key_pressed(VK_RWIN),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::ModifierKey;

    pub(super) fn is_modifier_pressed(_key: ModifierKey) -> bool {
        // No native helper exists for other platforms (TS returns false too).
        false
    }
}

/// Whether the given modifier is currently held down.
///
/// Returns `false` when no native support exists, exactly like the TS version
/// when the prebuilt addon cannot be loaded.
pub fn is_native_modifier_pressed(key: ModifierKey) -> bool {
    platform::is_modifier_pressed(key)
}
