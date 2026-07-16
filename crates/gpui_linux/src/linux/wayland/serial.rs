use collections::HashMap;

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) enum SerialKind {
    DataDevice,
    InputMethod,
    MouseEnter,
    MousePress,
    KeyPress,
}

#[derive(Debug)]
struct SerialData {
    serial: u32,
    observed_at: u64,
}

impl SerialData {
    fn new(value: u32, observed_at: u64) -> Self {
        Self {
            serial: value,
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
            .insert(kind, SerialData::new(value, self.observation_count));
    }

    /// Returns the latest tracked serial of the provided [`SerialKind`]
    ///
    /// Will return 0 if not tracked.
    pub fn get(&self, kind: SerialKind) -> u32 {
        self.serials
            .get(&kind)
            .map(|serial_data| serial_data.serial)
            .unwrap_or(0)
    }

    /// Returns the most recently observed serial of the provided [`SerialKind`]s.
    ///
    /// Comparing serial values is not sufficient because Wayland serials are
    /// 32-bit values and can wrap while the client is running.
    pub fn latest_of(&self, kinds: &[SerialKind]) -> u32 {
        kinds
            .iter()
            .filter_map(|kind| self.serials.get(kind))
            .max_by_key(|serial_data| serial_data.observed_at)
            .map(|serial_data| serial_data.serial)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_of_uses_observation_order_after_serial_wraparound() {
        let mut tracker = SerialTracker::new();
        tracker.update(SerialKind::KeyPress, u32::MAX);
        tracker.update(SerialKind::MousePress, 1);

        assert_eq!(
            tracker.latest_of(&[SerialKind::KeyPress, SerialKind::MousePress]),
            1
        );
    }

    #[test]
    fn latest_of_ignores_newer_unrequested_serial_kinds() {
        let mut tracker = SerialTracker::new();
        tracker.update(SerialKind::MousePress, 10);
        tracker.update(SerialKind::InputMethod, 20);
        tracker.update(SerialKind::DataDevice, 30);

        assert_eq!(
            tracker.latest_of(&[SerialKind::KeyPress, SerialKind::MousePress]),
            10
        );
    }
}
