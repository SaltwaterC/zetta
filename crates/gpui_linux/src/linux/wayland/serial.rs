use collections::HashMap;

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) enum SerialKind {
    DataDevice,
    InputMethod,
    MouseEnter,
    MousePress,
    KeyPress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Serial(u32);

impl Serial {
    pub(super) fn as_raw(self) -> u32 {
        self.0
    }
}

/// A serial produced by an eligible keyboard or pointer press.
///
/// Wayland accepts these serials for selection ownership and interactive
/// requests such as popup grabs. Keeping them distinct from other protocol
/// serials prevents a data-device, IME, or pointer-enter event from being used
/// accidentally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SelectionSerial(Serial);

impl SelectionSerial {
    pub(super) fn as_raw(self) -> u32 {
        self.0.as_raw()
    }
}

#[derive(Debug)]
struct SerialData {
    serial: Serial,
    observed_at: u64,
}

impl SerialData {
    fn new(serial: Serial, observed_at: u64) -> Self {
        Self {
            serial,
            observed_at,
        }
    }
}

#[derive(Debug)]
/// Helper for tracking of different serial kinds.
pub(crate) struct SerialTracker {
    serials: HashMap<SerialKind, SerialData>,
    observation_count: u64,
}

impl SerialTracker {
    pub fn new() -> Self {
        Self {
            serials: HashMap::default(),
            observation_count: 0,
        }
    }

    pub fn update(&mut self, kind: SerialKind, value: u32) {
        self.observation_count = self.observation_count.wrapping_add(1);
        self.serials
            .insert(kind, SerialData::new(Serial(value), self.observation_count));
    }

    /// Returns the latest tracked serial of the provided [`SerialKind`]
    ///
    /// Returns a serial with a raw value of 0 if the kind has not been tracked.
    pub fn get(&self, kind: SerialKind) -> Serial {
        self.serials
            .get(&kind)
            .map(|serial_data| serial_data.serial)
            .unwrap_or(Serial(0))
    }

    /// Returns the most recently observed serial of the provided [`SerialKind`]s.
    ///
    /// Comparing serial values is not sufficient because Wayland serials are
    /// 32-bit values and can wrap while the client is running.
    fn latest_of(&self, kinds: &[SerialKind]) -> Option<Serial> {
        kinds
            .iter()
            .filter_map(|kind| self.serials.get(kind))
            .max_by_key(|serial_data| serial_data.observed_at)
            .map(|serial_data| serial_data.serial)
    }

    /// Returns the newest keyboard or pointer press serial, if one has been
    /// observed.
    ///
    /// Arrival order is intentional: Wayland serials are 32-bit values and
    /// comparing their numeric values breaks after compositor rollover.
    /// `Some(Serial(0))` is distinct from no eligible input having arrived.
    pub fn selection_serial(&self) -> Option<SelectionSerial> {
        self.latest_of(&[SerialKind::KeyPress, SerialKind::MousePress])
            .map(SelectionSerial)
    }
}

#[cfg(test)]
#[path = "tests/serial.rs"]
mod tests;
