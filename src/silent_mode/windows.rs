//! Windows' Do Not Disturb, read from the shell's quiet-hours WNF state.

use super::{SystemSilentState, classify_windows_dnd_query};

#[repr(C)]
struct WnfStateName {
    data: [u32; 2],
}

type NtQueryWnfStateData = unsafe extern "system" fn(
    state_name: *const WnfStateName,
    type_id: *const std::ffi::c_void,
    explicit_scope: *const std::ffi::c_void,
    change_stamp: *mut u32,
    buffer: *mut std::ffi::c_void,
    buffer_size: *mut u32,
) -> i32;

// This state is updated by the Windows shell whenever the live Do Not Disturb
// profile changes. Its four-byte payload is 0 for off, 1 for priority-only,
// and 2 for alarms-only.
const WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED: WnfStateName = WnfStateName {
    data: [0xA3BF_1C75, 0x0D83_063E],
};

pub(super) fn detect_windows_system_silent_state() -> SystemSilentState {
    let Some(query) = nt_query_wnf_state_data() else {
        return SystemSilentState::Unknown;
    };
    let mut change_stamp = 0;
    let mut profile = 0_u32;
    let mut buffer_size = size_of::<u32>() as u32;
    // SAFETY: The cached address is resolved from ntdll under the exact
    // NtQueryWnfStateData ABI. Every pointer is either null for an optional
    // parameter or points to writable storage of the advertised size.
    let status = unsafe {
        query(
            &WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED,
            std::ptr::null(),
            std::ptr::null(),
            &mut change_stamp,
            std::ptr::addr_of_mut!(profile).cast(),
            &mut buffer_size,
        )
    };
    classify_windows_dnd_query(Some((status, buffer_size, profile)))
}

fn nt_query_wnf_state_data() -> Option<NtQueryWnfStateData> {
    use std::sync::OnceLock;

    static QUERY: OnceLock<Option<NtQueryWnfStateData>> = OnceLock::new();
    *QUERY.get_or_init(|| {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        use windows::core::{s, w};

        // SAFETY: ntdll is loaded for every Windows process. GetProcAddress
        // returns None when this private API is absent, which is cached as a
        // conservative Unknown result by the caller.
        let module = unsafe { GetModuleHandleW(w!("ntdll.dll")) }.ok()?;
        let address = unsafe { GetProcAddress(module, s!("NtQueryWnfStateData")) }?;
        // SAFETY: The named ntdll export has the NtQueryWnfStateData ABI
        // declared above. The pointer remains valid because ntdll cannot be
        // unloaded during the process lifetime.
        Some(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, NtQueryWnfStateData>(
                address,
            )
        })
    })
}
