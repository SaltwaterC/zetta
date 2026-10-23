use super::*;
use std::mem::{offset_of, size_of};
#[test]
#[cfg(target_pointer_width = "64")]
fn wire_layout_matches_condrv_x64() {
    assert_eq!(size_of::<Identifier>(), 8);
    assert_eq!(size_of::<Descriptor>(), 40);
    assert_eq!(offset_of!(Descriptor, function), 24);
    assert_eq!(size_of::<Buffer>(), 16);
    assert_eq!(size_of::<IoStatus>(), 16);
    assert_eq!(size_of::<Completion>(), 40);
    assert_eq!(offset_of!(Completion, write), 24);
    assert_eq!(size_of::<Operation>(), 24);
    assert_eq!(offset_of!(Packet, payload), 40);
}
#[test]
fn negative_status_preserves_the_ntstatus_bits() {
    let value = Completion::new(Identifier::default(), INVALID_PARAMETER, 0, &mut []);
    assert_eq!(value.status.status, 0xc000000d);
}
#[test]
fn an_allocation_too_large_to_make_is_an_out_of_memory_error() {
    let error = try_vec(0u64, usize::MAX).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::OutOfMemory);
}
