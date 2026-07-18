use super::*;

pub(crate) const SERIAL_READ_TIMEOUT: Duration = Duration::from_millis(100);
const COMMON_BAUD_RATES: [u32; 12] = [
    300, 1_200, 2_400, 4_800, 9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SerialField {
    Device,
    BaudRate,
    DataBits,
    Parity,
    StopBits,
    FlowControl,
}

impl SerialField {
    const ALL: [Self; 6] = [
        Self::Device,
        Self::BaudRate,
        Self::DataBits,
        Self::Parity,
        Self::StopBits,
        Self::FlowControl,
    ];

    pub(crate) fn adjacent(self, reverse: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|field| *field == self)
            .unwrap_or(0);
        let index = if reverse {
            index.checked_sub(1).unwrap_or(Self::ALL.len() - 1)
        } else {
            (index + 1) % Self::ALL.len()
        };
        Self::ALL[index]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SerialDevice {
    pub(crate) port_name: String,
    pub(crate) description: Option<String>,
}

impl From<serialport::SerialPortInfo> for SerialDevice {
    fn from(info: serialport::SerialPortInfo) -> Self {
        let description = match info.port_type {
            serialport::SerialPortType::UsbPort(usb) => {
                let product = usb.product.filter(|value| !value.is_empty());
                let manufacturer = usb.manufacturer.filter(|value| !value.is_empty());
                product
                    .or(manufacturer)
                    .or_else(|| Some(format!("USB {:04x}:{:04x}", usb.vid, usb.pid)))
            }
            serialport::SerialPortType::BluetoothPort => Some("Bluetooth".to_owned()),
            serialport::SerialPortType::PciPort => Some("PCI".to_owned()),
            serialport::SerialPortType::Unknown => None,
        };
        Self {
            port_name: info.port_name,
            description,
        }
    }
}

pub(crate) struct SerialConsolePrompt {
    pub(crate) devices: Vec<SerialDevice>,
    pub(crate) selected_device: usize,
    pub(crate) baud_rate: String,
    pub(crate) baud_cursor: usize,
    pub(crate) field: SerialField,
    pub(crate) data_bits: serialport::DataBits,
    pub(crate) parity: serialport::Parity,
    pub(crate) stop_bits: serialport::StopBits,
    pub(crate) flow_control: serialport::FlowControl,
    pub(crate) loading: bool,
    pub(crate) connecting: bool,
    pub(crate) error: Option<String>,
}

impl Default for SerialConsolePrompt {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            selected_device: 0,
            baud_rate: "115200".to_owned(),
            baud_cursor: 6,
            field: SerialField::Device,
            data_bits: serialport::DataBits::Eight,
            parity: serialport::Parity::None,
            stop_bits: serialport::StopBits::One,
            flow_control: serialport::FlowControl::None,
            loading: true,
            connecting: false,
            error: None,
        }
    }
}

impl SerialConsolePrompt {
    pub(crate) fn framing_label(&self) -> String {
        format!(
            "{}{}{}",
            data_bits_label(self.data_bits),
            parity_short_label(self.parity),
            stop_bits_label(self.stop_bits)
        )
    }

    pub(crate) fn cycle_current_value(&mut self, reverse: bool) {
        match self.field {
            SerialField::Device => {
                if !self.devices.is_empty() {
                    self.selected_device =
                        cycle_index(self.selected_device, self.devices.len(), reverse);
                }
            }
            SerialField::BaudRate => {
                let current = self.baud_rate.parse::<u32>().unwrap_or(0);
                let index = match COMMON_BAUD_RATES.binary_search(&current) {
                    Ok(index) => cycle_index(index, COMMON_BAUD_RATES.len(), reverse),
                    Err(index) if reverse => index.saturating_sub(1),
                    Err(index) => index.min(COMMON_BAUD_RATES.len() - 1),
                };
                self.baud_rate = COMMON_BAUD_RATES[index].to_string();
                self.baud_cursor = self.baud_rate.len();
            }
            SerialField::DataBits => {
                const VALUES: [serialport::DataBits; 4] = [
                    serialport::DataBits::Five,
                    serialport::DataBits::Six,
                    serialport::DataBits::Seven,
                    serialport::DataBits::Eight,
                ];
                self.data_bits = cycle_value(self.data_bits, &VALUES, reverse);
            }
            SerialField::Parity => {
                const VALUES: [serialport::Parity; 3] = [
                    serialport::Parity::None,
                    serialport::Parity::Odd,
                    serialport::Parity::Even,
                ];
                self.parity = cycle_value(self.parity, &VALUES, reverse);
            }
            SerialField::StopBits => {
                const VALUES: [serialport::StopBits; 2] =
                    [serialport::StopBits::One, serialport::StopBits::Two];
                self.stop_bits = cycle_value(self.stop_bits, &VALUES, reverse);
            }
            SerialField::FlowControl => {
                const VALUES: [serialport::FlowControl; 3] = [
                    serialport::FlowControl::None,
                    serialport::FlowControl::Software,
                    serialport::FlowControl::Hardware,
                ];
                self.flow_control = cycle_value(self.flow_control, &VALUES, reverse);
            }
        }
    }
}

fn cycle_index(index: usize, len: usize, reverse: bool) -> usize {
    if reverse {
        index.checked_sub(1).unwrap_or(len - 1)
    } else {
        (index + 1) % len
    }
}

fn cycle_value<T: Copy + PartialEq>(current: T, values: &[T], reverse: bool) -> T {
    values[cycle_index(
        values
            .iter()
            .position(|value| *value == current)
            .unwrap_or(0),
        values.len(),
        reverse,
    )]
}

pub(crate) fn data_bits_label(value: serialport::DataBits) -> &'static str {
    match value {
        serialport::DataBits::Five => "5",
        serialport::DataBits::Six => "6",
        serialport::DataBits::Seven => "7",
        serialport::DataBits::Eight => "8",
    }
}

pub(crate) fn parity_label(value: serialport::Parity) -> &'static str {
    match value {
        serialport::Parity::None => "None",
        serialport::Parity::Odd => "Odd",
        serialport::Parity::Even => "Even",
    }
}

fn parity_short_label(value: serialport::Parity) -> &'static str {
    match value {
        serialport::Parity::None => "N",
        serialport::Parity::Odd => "O",
        serialport::Parity::Even => "E",
    }
}

pub(crate) fn stop_bits_label(value: serialport::StopBits) -> &'static str {
    match value {
        serialport::StopBits::One => "1",
        serialport::StopBits::Two => "2",
    }
}

pub(crate) fn flow_control_label(value: serialport::FlowControl) -> &'static str {
    match value {
        serialport::FlowControl::None => "None",
        serialport::FlowControl::Software => "Software (XON/XOFF)",
        serialport::FlowControl::Hardware => "Hardware (RTS/CTS)",
    }
}

pub(crate) struct OpenSerialConnection {
    pub(crate) reader: Box<dyn serialport::SerialPort>,
    pub(crate) writer: Box<dyn serialport::SerialPort>,
}

#[derive(Clone)]
pub(crate) struct SerialConnectionSettings {
    pub(crate) port_name: String,
    pub(crate) baud_rate: u32,
    pub(crate) data_bits: serialport::DataBits,
    pub(crate) parity: serialport::Parity,
    pub(crate) stop_bits: serialport::StopBits,
    pub(crate) flow_control: serialport::FlowControl,
}

pub(crate) fn open_serial_connection(
    settings: &SerialConnectionSettings,
) -> Result<OpenSerialConnection> {
    let writer = serialport::new(&settings.port_name, settings.baud_rate)
        .data_bits(settings.data_bits)
        .parity(settings.parity)
        .stop_bits(settings.stop_bits)
        .flow_control(settings.flow_control)
        .timeout(SERIAL_READ_TIMEOUT)
        .open()
        .with_context(|| format!("opening serial device {}", settings.port_name))?;
    let reader = writer
        .try_clone()
        .with_context(|| format!("cloning serial device {} for reading", settings.port_name))?;
    Ok(OpenSerialConnection { reader, writer })
}

#[cfg(test)]
#[path = "tests/serial_console.rs"]
mod tests;
