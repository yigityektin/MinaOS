use crate::{block, block::setup_block_device, page::PAGE_SIZE};
use crate::rng::setup_entropy_device;
use crate::{gpu, gpu::setup_gpu_device};
use crate::{input, input::setup_input_device};
use core::men::size_of;

pub const IO_F_RING_INDIRECT_DESC: u32 = 28;
pub const IO_F_RING_EVENT_IDX: u32 = 29;
pub const IO_F_VERSION_1: u32 = 32;

pub const IO_DESC_F_NEXT: u16 = 1;
pub const IO_DESC_F_WRITE: u16 = 2;
pub const IO_DESC_F_INDIRECT: u16 = 4;

pub const IO_AVAIL_F_NO_INTERRUPT: u16 = 1;
pub const IO_USED_F_NO_NOTIFY: u16 = 1;
pub const IO_RING_SIZE: usize = 1 << 7;

#[repr(C)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
pub struct Available {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; IO_RING_SIZE],
    pub event: u16,
}

#[repr(C)]
pub struct UsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C)]
pub struct Used {
    pub flags: u16,
    pub idx: u16,
    pub ring: [UsedElem; IO_RING_SIZE],
    pub event: u16,
}

#[repr(C)]
pub struct Queue {
    pub desc: [Descriptor; IO_RING_SIZE],
    pub avail: Available,
    pub padding0: [u8; PAGE_SIZE - size_of::<Descriptor>() * IO_RING_SIZE - size_of::<Available>()],
    pub used: Used,
}

#[repr(usize)]
pub enum MmioOffsets {
    MagicValue = 0x000,
    Version = 0x004,
    DeviceId = 0x008,
    VendorId = 0x00c,
    HostFeatures = 0x010,
    HostFeaturesSel = 0x014,
    GuestFeatures = 0x020,
    GuestFeaturesSel = 0x024,
    GuestPageSize = 0x028,
    QueueSel = 0x030,
    QueueNumMax = 0x034,
    QueueNum = 0x038,
    QueueAlign = 0x03c,
    QueuePfn = 0x040,
    QueueNotify = 0x050,
    InterruptStatus = 0x060,
    InterruptAck = 0x064,
    Status = 0x070,
    Config = 0x100,
}

#[repr(C)]
pub struct MmioDevice {
    magic_value: u32,
    version: u32,
    device_id: u32,
    vendor_id: u32,
    host_features: u32,
    host_features_sel: u32,
    rsv1: [u8; 8],
    guest_features: u32,
    guest_features_sel: u32,
    guest_page_size: u32,
    rsv2: [u8; 4],
    queue_sel: u32,
    queue_num_max: u32,
    queue_num: u32,
    queue_align: u32,
    queue_pfn: u64,
    rsv3: [u8; 8],
    queue_notify: u32,
    rsv4: [u8, 12],
    interrupt_status: u32,
    interrupt_ack: u32,
    rsv5: [u8; 8],
    status: u32,
}

#[repr(usize)]
pub enum DeviceTypes {
    None = 0,
    Network = 1,
    Block = 2,
    Console = 3,
    Entropy = 4,
    Gpu = 16,
    Input = 18,
    Memory = 24,
}

impl MmioOffsets {
    pub fn val(self) -> usize {
        self as usize
    }

    pub fn scaled(self, scale: usize) -> usize {
        self.val() / scale
    }

    pub fn scale32(self) -> usize {
        self.scaled(4)
    }
}

pub enum StatusField {
    Acknowledge = 1,
    Driver = 2,
    Failed = 128,
    FeaturesOk = 8,
    DriverOk = 4,
    DeviceNeedsReset = 64,
}

impl StatusField {
    pub fn val(self) -> usize {
        self as usize
    }

    pub fn val32(self) -> u32 {
        self as u32
    }

    pub fn test(sf: u32, bit: StatusField) -> bool {
        sf & bit.val32() != 0
    }

    pub fn is_failed(sf: u32) -> bool {
        StatusField::test(sf, StatusField::Failed)
    }

    pub fn needs_reset(sf: u32) -> bool {
        StatusField::test(sf, StatusField::DeviceNeedsReset)
    }

    pub fn driver_ok(sf: u32) -> bool {
        StatusField::test(sf, StatusField::DriverOk)
    }

    pub fn features_ok(sf: u32) -> bool {
        StatusField::test(sf, StatusField::FeaturesOk)
    }
}

pub const MMIO_IO_START: usize = 0x1000_1000;
pub const MMIO_IO_END: usize = 0x1000_8000;
pub const MMIO_IO_STRIDE: usize = 0x1000;
pub const MMIO_IO_MAGIC: u32 = 0x74_72_69_76;

pub struct IoDevice {
    pub devtype: DeviceTypes,
}

impl IoDevice {
    pub const fn new() -> Self {
        IoDevice { devtype: DeviceTypes::None, }
    }

    pub const fn new_with(devtype: DeviceTypes) -> Self {
        IoDevice {devtype}
    }
}

static mut IO_DEVICES: [Option<IoDevice>; 8] = [None, None, None, None, None, None, None, None];

pub fn probe() {
    for addr in (MMIO_IO_START..=MMIO_IO_END).step_by(MMIO_IO_STRIDE) {
        print!("Io probing 0x{:08x}.", addr);
        let magicvalue;
        let deviceid;
        let ptr = addr as *mut u32;
        unsafe {
            magicvalue = ptr.read_volatile();
            deviceid = ptr.add(2).read_volatile();

            if MMIO_IO_MAGIC != magicvalue {
                println!("not io.");
            } else if 0 == deviceid {
                println!("not connected.");
            } else {
                match deviceid {
                    1 => {
                        print!("network device...");
                        if false == setup_network_device(ptr) {
                            println!("setup failed.");
                        } else {
                            println!("setup succeeded.");
                        }
                    },
                    2 => {
                        print!("block device...");
                        if false == setup_block_device(ptr) {
                            println!("setup failed.");
                        } else {
                            let idx = (addr - MMIO_IO_START) >> 12;
                            unsafe {
                                IO_DEVICES[idx] = Some(IoDevice::new_with(DeviceTypes::Block));
                            }
                            println!("setup succeeded.");
                        }
                    },
                    4 => {
                        print!("entropy device...");
                        if false == setup_entropy_device(ptr) {
                            println!("setup failed.");
                        } else {
                            println!("setup succeeded.");
                        }
                    },
                    16 => {
                        print!("GPU device...");
                        if false == setup_gpu_device(ptr) {
                            println!("setup failed.");
                        } else {
                            let idx = (addr - MMIO_IO_START) >> 12;
                            unsafe {
                                IO_DEVICES[idx] = Some(IoDevice::new_with(DeviceTypes::Gpu));
                            }
                            println!("setup succeeded.");
                        }
                    },
                    18 => {
                        print!("input device...");
                        if false == setup_input_device(ptr) {
                            println!("setup failed.");
                        } else {
                            let idx = (addr - MMIO_IO_START) >> 12;
                            unasfe {
                                IO_DEVICES[idx] = Some(IoDevice::new_with(DeviceTypes::Input));
                            }
                            println!("setup succeeded.");
                        }
                    },
                    _ => println!("unknown device type."),
                
            }
        }
    }
}

pub fn setup_network_device(_ptr: *mut u32) -> bool {
    false
}

pub fn handle_interrupt(interrupt: u32) {
    let idx = interrupt as usize - 1;
    unsafe {
        if let Some(vd) = &IO_DEVICES[idx] {
            match vd.devtype {
                DeviceTypes::Block => {
                    block::handle_interrupt(idx);
                },
                DeviceTypes::Gpu => {
                    gpu::handle_interrupt(idx);
                },
                DeviceTypes::Input => {
                    input::handle_interrupt(idx);
                },
                _ => {
                    println!("Invalid device generated interrupt.");
                },
            }
        }
        else {
            println!("Spurious interrupt {}", interrupt);
        }
    }
}