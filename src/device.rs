
use std::time::Duration;

use byteorder::{LE, BE, ByteOrder, ReadBytesExt, WriteBytesExt};

use embedded_hal::digital::v2::{InputPin, OutputPin};
use embedded_hal::blocking::spi::{Transfer, Write};

use libusb::{Device, DeviceDescriptor, DeviceHandle, Direction, TransferType};

use crate::Error;

pub struct Cp2130<'a> {
    _device: Device<'a>,
    handle: DeviceHandle<'a>,
    info: Info,
    endpoints: Endpoints,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    manufacturer: String,
    product: String,
    serial: String,
}

#[derive(Debug)]
pub struct Endpoints {
    control: Endpoint,
    read: Endpoint,
    write: Endpoint,
}

#[derive(Debug)]
struct Endpoint {
    config: u8,
    iface: u8,
    setting: u8,
    address: u8
}

pub enum Commands {
    GetClockDivider = 0x46,
    GetEventCounter = 0x44,
    GetFullThreshold = 0x34,
    GetGpioChipSelect = 0x24,
    GetGpioModeAndLevel = 0x22,
    GetGpioValues = 0x20,
    GetRtrState = 0x36,
    GetSpiWord = 0x30,
    GetSpiDelay = 0x32,
    GetReadOnlyVersion = 0x11,
    ResetDevice = 0x10,
    SetClockDivider = 0x47,
    SetEventCOunter = 0x45,
    SetFullThreshold = 0x35,
    SetGpioChipSelect = 0x25,
    SetGpioModeAndLevel = 0x23,
    SetGpioValues = 0x21,
    SetRtrStop = 0x37,
    SetSpiWord = 0x31,
    SetSpiDelay = 0x33,
}

pub const VID: u16 = 0x10c4;
pub const PID: u16 = 0x87a0;

bitflags!(
    struct RequestType: u8 {
        const HOST_TO_DEVICE = 0b0000_0000;
        const DEVICE_TO_HOST = 0b1000_0000;

        const TYPE_STANDARD = 0b0000_0000;
        const TYPE_CLASS =    0b0010_0000;
        const TYPE_VENDOR =   0b0100_0000;

        const RECIPIENT_DEVICE =    0b0000_0000;
        const RECIPIENT_INTERFACE = 0b0000_0001;
        const RECIPIENT_ENDPOINT =  0b0000_0010;
        const RECIPIENT_OTHER =     0b0000_0011;
    }
);



pub enum TransferCommand {
    Read        = 0x00,
    Write       = 0x01,
    WriteRead   = 0x02,
    ReadWithRTR = 0x04,
}


impl <'a> Cp2130<'a> {
    /// Create a new CP2130 instance from a libusb device and descriptor
    pub fn new(device: Device<'a>, descriptor: DeviceDescriptor) -> Result<Self, Error> {
        let timeout = Duration::from_millis(200);
        
        // Fetch device handle
        let mut handle = match device.open() {
            Ok(v) => v,
            Err(e) => {
                error!("Opening device: {}", e);
                return Err(Error::Usb(e))
            }
        };


        // Reset device
        handle.reset()?;

        // Fetch base configuration
        let languages = handle.read_languages(timeout)?;
        let active_config = handle.active_configuration()?;

        debug!("Active configuration: {}", active_config);
        debug!("Languages: {:?}", languages);

        // Check a language is available
        if languages.len() == 0 {
            return Err(Error::NoLanguages)
        }

        // Fetch information
        let language = languages[0];
        let manufacturer = handle.read_manufacturer_string(language, &descriptor, timeout)?;
        let product = handle.read_product_string(language, &descriptor, timeout)?;
        let serial = handle.read_serial_number_string(language, &descriptor, timeout)?;
        let info = Info{manufacturer, product, serial};

        // Check at least one configuration exists
        if descriptor.num_configurations() != 1 {
            error!("Unexpected number of configurations");
            return Err(Error::Configurations)
        }

        // Connect to endpoints
        let config_desc = device.config_descriptor(0)?;
        
        let (mut control, mut write, mut read) = (None, None, None);

        for interface in config_desc.interfaces() {
            for interface_desc in interface.descriptors() {
                for endpoint_desc in interface_desc.endpoint_descriptors() {

                    // Create an endpoint container
                    let e = Endpoint {
                        config: config_desc.number(),
                        iface: interface_desc.interface_number(),
                        setting: interface_desc.setting_number(),
                        address: endpoint_desc.address(),
                    };

                    debug!("Endpoint: {:?}", e);

                    // Find the relevant endpoints
                    match (endpoint_desc.transfer_type(), endpoint_desc.direction()) {
                        (TransferType::Control, _) => control = Some(e),
                        (TransferType::Bulk, Direction::In) => read = Some(e),
                        (TransferType::Bulk, Direction::Out) => write = Some(e),
                        (_, _) => continue,
                    }
                }
            }
        }

        // Configure endpoints
        let control = Endpoint {
            config: 1,
            iface: 0,
            setting: 0,
            address: 0,
        };
        //control.configure(&mut handle)?;

        let write = match write {
            Some(c) => {
                debug!("Located write endpoint");
                c
            },
            None => {
                error!("No write endpoint found");
                return Err(Error::Endpoint)
            }
        };
        write.configure(&mut handle)?;

        let read = match read {
            Some(c) => {
                debug!("Located read endpoint");
                c
            },
            None => {
                error!("No read endpoint found");
                return Err(Error::Endpoint)
            }
        };
        read.configure(&mut handle)?;

        let endpoints = Endpoints{control, write, read};

        // Create device
        Ok(Self{_device: device, handle, info, endpoints})
    }

    /// Fetch information for the connected device
    pub fn info(&self) -> Info {
        self.info.clone()
    }

    pub fn spi_read(&mut self, buff: &mut [u8]) -> Result<usize, Error> {
        let mut cmd = [0u8; 8];
        cmd[2] = TransferCommand::Read as u8;
        LE::write_u32(&mut cmd[4..], buff.len() as u32);

        self.handle.write_bulk(
            self.endpoints.write.address,
            &cmd,
            Duration::from_millis(200),
        )?;

        // TODO: loop for > 64-byte packets
        let mut index = 0;

        while index < buff.len() {
            let remainder = if buff.len() > index + 64 {
                64
            } else {
                buff.len() - index
            };

            let n = self.handle.read_bulk(
                self.endpoints.write.address,
                &mut buff[index..index+remainder],
                Duration::from_millis(200),
            )?;

            index += n;
        }

        Ok(index)
    }

    pub fn spi_write(&mut self, buff: &[u8]) -> Result<(), Error> {

        let mut cmd = vec![0u8; buff.len() + 8];

        cmd[2] = TransferCommand::Write as u8;
        LE::write_u32(&mut cmd[4..], buff.len() as u32);
        (&mut cmd[8..]).copy_from_slice(buff);

        self.handle.write_bulk(
            self.endpoints.write.address,
            &cmd,
            Duration::from_millis(200),
        )?;

        Ok(())
    }

    pub fn spi_write_read(&mut self, buff_out: &[u8], buff_in: &mut [u8]) -> Result<usize, Error> {

        let mut cmd = vec![0u8; buff_out.len() + 8];

        cmd[2] = TransferCommand::WriteRead as u8;
        LE::write_u32(&mut cmd[4..], buff_out.len() as u32);
        (&mut cmd[8..]).copy_from_slice(buff_out);

        self.handle.write_bulk(
            self.endpoints.write.address,
            &cmd,
            Duration::from_millis(200),
        )?;

        // TODO: loop for > 64-byte packets
        let n = self.handle.read_bulk(
            self.endpoints.write.address,
            buff_in,
            Duration::from_millis(200),
        )?;

        Ok(n)
    }

    /// Fetch the chip version
    pub fn version(&mut self) -> Result<u16, Error> {
        let mut buff = [0u8; 2];

        self.handle.read_control(
            (RequestType::DEVICE_TO_HOST | RequestType::TYPE_VENDOR).bits(), 
            Commands::GetReadOnlyVersion as u8,
            0, 0,
            &mut buff,
            Duration::from_millis(200)
        )?;

        let version = LE::read_u16(&buff);

        Ok(version)
    }

    pub fn set_gpio_mode_level(&mut self, pin: u8, mode: GpioMode, level: GpioLevel) -> Result<(), Error> {
        assert!(pin <= 10);
        
        let cmd = [
            pin,
            mode as u8,
            level as u8,
        ];

        self.handle.write_control(
            (RequestType::HOST_TO_DEVICE | RequestType::TYPE_VENDOR).bits(), 
            Commands::SetGpioModeAndLevel as u8,
            0, 0,
            &cmd,
            Duration::from_millis(200)
        )?;

        Ok(())
    }

    pub fn get_gpio_values(&mut self) -> Result<GpioLevels, Error> {
        let mut buff = [0u8; 2];

        self.handle.read_control(
            (RequestType::DEVICE_TO_HOST | RequestType::TYPE_VENDOR).bits(), 
            Commands::GetGpioValues as u8,
            0, 0,
            &mut buff,
            Duration::from_millis(200)
        )?;

        // Inexplicably big endian here
        let values = BE::read_u16(&buff);

        Ok(GpioLevels::from_bits_truncate(values))
    }

    pub fn get_gpio_level(&mut self, pin: u8) -> Result<bool, Error> {
        assert!(pin <= 10);

        let levels = self.get_gpio_values()?;

        let v = match pin {
            0 => levels.contains(GpioLevels::GPIO_0),
            1 => levels.contains(GpioLevels::GPIO_1),
            2 => levels.contains(GpioLevels::GPIO_2),
            3 => levels.contains(GpioLevels::GPIO_3),
            4 => levels.contains(GpioLevels::GPIO_4),
            5 => levels.contains(GpioLevels::GPIO_5),
            6 => levels.contains(GpioLevels::GPIO_6),
            7 => levels.contains(GpioLevels::GPIO_7),
            8 => levels.contains(GpioLevels::GPIO_8),
            9 => levels.contains(GpioLevels::GPIO_9),
            10 => levels.contains(GpioLevels::GPIO_10),
            _ => panic!("invalid pin {}", pin),
        };

        Ok(v)
    }

}


bitflags!(
    /// Gpio PIN masks for multiple pin operations
    /// The endianness of this varies depending on where it is used...
    pub struct GpioLevels: u16 {
        const GPIO_10 = (1 << 14);
        const GPIO_9  = (1 << 13);
        const GPIO_8  = (1 << 12);
        const GPIO_7  = (1 << 11);
        const GPIO_6  = (1 << 10);
        const GPIO_5  = (1 << 8);

        const GPIO_4  = (1 << 7);
        const GPIO_3  = (1 << 6);
        const GPIO_2  = (1 << 5);
        const GPIO_1  = (1 << 4);
        const GPIO_0  = (1 << 3);
    }
);

pub enum GpioMode {
    Input = 0x00,
    OpenDrain = 0x01,
    PushPull = 0x02,
}

pub enum GpioLevel {
    Low = 0x00,
    High = 0x01,
}

impl Endpoint {
    fn configure(&self, handle: &mut DeviceHandle) -> Result<(), Error> {
        // Detach kernel driver if required
        if handle.kernel_driver_active(self.iface)? {
            debug!("Detaching kernel driver");
            handle.detach_kernel_driver(self.iface)?;
            // TODO: track this and re-enable on closing?
        }
    
        // Configure endpoint
        debug!("Setting configuration");
        handle.set_active_configuration(self.config)?;
        //debug!("Claiming interface");
        //handle.claim_interface(self.iface)?;
        //debug!("Setting alternate setting");
        //handle.set_alternate_setting(self.iface, self.setting)?;

        Ok(())
    }
}

impl <'a> Transfer<u8> for Cp2130<'a> {
    type Error = Error;

    fn transfer<'w>(&mut self, _words: &'w mut [u8] ) -> Result<&'w [u8], Self::Error> {
        unimplemented!()
    }
}

impl <'a> Write<u8> for Cp2130<'a> {
    type Error = Error;

    fn write(&mut self, _words: &[u8] ) -> Result<(), Self::Error> {
        unimplemented!()
    }
}


pub struct Gpio {

}

impl InputPin for Gpio {
    type Error = Error;

    fn is_high(&self) -> Result<bool, Self::Error> {
        unimplemented!()
    }

    fn is_low(&self) -> Result<bool, Self::Error> {
        unimplemented!()
    }
}


impl OutputPin for Gpio {
    type Error = Error;

    fn set_high(&mut self) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn set_low(&mut self) -> Result<(), Self::Error> {
        unimplemented!()
    }
}
