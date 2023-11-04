use crate::{kmem::{kfree, kmalloc},
            page::{zalloc, PAGE_SIZE},
            process::{add_kernel_process_args, get_by_pid, set_running, set_waiting},
            io,
        io::{Descriptor, MmioOffsets, Queue, StatusField, IO_RING_SIZE}};

use core::mem::size_of;
use alloc::boxed::Box;

#[repr(C)]
pub struct Geometry {
    cylinders: u16,
    heads: u8,
    sectors: u8,
}

#[repr(C)]
pub struct Topology {
    physical_block_exp: u8,
    alignment_offset: u8,
    min_io_size: u16,
    opt_io_size: u32,
}

#[repr(C)]
pub struct Config {
    capacity: u64,
    size_max: u32,
    seg_max: u32,
    geometry: Geometry,
    blk_size: u32,
    topology: Topology,
    writeback: u8,
    unused0: [u8; 3],
    max_discard_sector: u32,
    max_discard_seg: u32,
    discard_sector_alignment: u32,
    max_write_zeroes_sectors: u32,
    max_write_zeroes_seg: u32,
    write_zeroes_may_unmap: u8,
    unused1: [u8, 3],
}

#[repr(C)]
pub struct Header {
    blktype: u32,
    reserved: u32,
    sector: u64,
}

#[repr(C)]
pub struct Data {
    data: *mut u8,
}

#[repr(C)]
pub struct Status {
    status: u8,
}

#[repr(C)]
pub struct Request {
    header: Header,
    data: Data,
    status: Status,
    head: u16,
    watcher: u16,
}

pub struct BlockDevice {
    queue: *mut Queue,
    dev: *mut u32,
    idx: u16,
    ack_used_idx: u16,
    read_only: bool,
}

//Type
pub const IO_BLK_T_IN: u32 = 0;
pub const IO_BLK_T_OUT: u32 = 1;
pub const IO_BLK_T_FLUSH: u32 = 4;
pub const IO_BLK_T_DISCARD: u32 = 11;
pub const IO_BLK_T_WRITE_ZEROES: u32 = 13;

//Status
pub const IO_BLK_S_OK: u8 = 0;
pub const IO_BLK_S_IOERR: u8 = 1;
pub const IO_BLK_S_UNSUPP: u8 = 2;

//Feature
pub const IO_BLK_F_SIZE_MAX: u32 = 1;
pub const IO_BLK_F_SEG_MAX: u32 = 2;
pub const IO_BLK_F_GEOMETRY: u32 = 4;
pub const IO_BLK_F_RO: u32 = 5;
pub const IO_BLK_F_BLK_SIZE: u32 = 6;
pub const IO_BLK_F_FLUSH: u32 = 9;
pub const IO_BLK_F_TOPOLOGY: u32 = 10;
pub const IO_BLK_F_CONFIG_WCE: u32 = 11;
pub const IO_BLK_F_DISCARD: u32 = 13;
pub const IO_BLK_F_WRITE_ZEROES: u32 = 14;

pub enum BlockErrors {
    Success = 0,
    BlockDeviceNotFound,
    InvalidArgument,
    ReadOnly,
}

static mut BLOCK_DEVICES: [Option<BlockDevice>; 8] = [None, None, None, None, None, None, None, None];

pub fn setup_block_device(ptr: *mut u32) -> bool {
    unsafe {
        let idx = (ptr as usize - io::MMIO_IO_START) >> 12;
        ptr.add(MmioOffsets::Status.scale32()).write_volatile(0);
        let mut status_bits = StatusField::Acknowledge.val32();
        ptr.add(MmioOffsets::Status.scale32()).write_volatile(status_bits);
        status_bits |= StatusField::DriverOk.val32();
        ptr.add(MmioOffsets::Status.scale32()).write_volatile(status_bits);

        let host_features = ptr.add(MmioOffsets::HostFeatures.scale32()).read_volatile();
        let guest_features = host_features & !(1 << IO_BLK_F_RO);
        let ro = host_features & (1 << IO_BLK_F_RO) != 0;

        ptr.add(MmioOffsets::GuestFeatures.scale32()).write_volatile(guest_features);
        status_bits |= StatusField::FeaturesOk.val32();
        ptr.add(MmioOffsets::Status.scale32()).write_volatile(status_bits);

        let status_ok = ptr.add(MmioOffsets::Status.scale32()).read_volatile();
        if false == StatusField::features_ok(status_ok) {
            print!("Features fail");
            ptr.add(MmioOffsets::Status.scale32()).write_volatile(StatusField::Failed.val32());
            return false;
        }

        let qnmax = ptr.add(MmioOffsets::QueueNumMax.scale32()).read_volatile();
        ptr.add(MmioOffsets::QueueNum.scale32()).write_volatile(IO_RING_SIZE as u32);
        if IO_RING_SIZE as u32 > qnmax {
            print!("Queue size fail");
            return false;
        }

        let num_pages = (size_of::<Queue>() + PAGE_SIZE - 1) / PAGE_SIZE;

        ptr.add(MmioOffsets::QueueSel.scale32()).write_volatile(0);

        let queue_ptr = zalloc(num_pages) as *mut Queue;
        let queue_pfn = queue_ptr as u32;
        ptr.add(MmioOffsets::GuestPageSize.scale32()).write_volatile(PAGE_SIZE as u32);

        ptr.add(MmioOffsets::QueuePfn.scale32()).write_volatile(queue_pfn / PAGE_SIZE as u32);

        let bd = BlockDevice {
            queue: queue_ptr,
            dev: ptr,
            idx: 0,
            ack_used_idx: 0,
            read_only: ro,
        };
        BLOCK_DEVICES[idx] = Some(bd);

        status_bits |= StatusField::DriverOk.val32();
        ptr.add(MmioOffsets::Status.scale32()).write_volatile(status_bits);

        true
    }
}

pub fn fill_next_descriptor(bd: &mut BlockDevice, desc: Descriptor) -> u16 {
    unsafe {
        bd.idx = (bd.idx + 1) % IO_RING_SIZE as u16;
        (*bd.queue).desc[bd.idx as usize] = desc;
        if (*bd.queue).desc[bd.idx as usize].flags & io::IO_DESC_F_NEXT != 0 {
            (*bd.queue).desc[bd.idx as usize].next = (bd.idx + 1) % IO_RING_SIZE as u16;
        }
        bd.idx  
    }
}

pub fn block_op(dev: usize, buffer: *mut u8, size: u32, offset: u64, write: bool, watcher: u16) -> Result<u32, BlockErrors> {
    unsafe {
        if let Some(bdev) = BLOCK_DEVICES[dev - 1].as_mut() {
            if bdev.read_only && write {
                return Err(BlockErrors::ReadOnly);
            }
            if size % 512 != 0 {
                return Err(BlockErrors::InvalidArgument);
            }
            let sector = offset / 512;
            let blk_request_size = size_of::<Request>();
            let blk_request = kmalloc(blk_request_size) as *mut Request;
            let desc = Descriptor {addr: &(*blk_request).header as *const Header as u64,
                                len: size_of::<Header>() as u32,
                                flags: io::IO_DESC_F_NEXT,
                            next: 0,};
            let head_idx = fill_next_descriptor(bdev, desc);
            (*blk_request).header.sector = sector;
            (*blk_request).header.blktype = if write {
                IO_BLK_T_OUT
            } else {
                IO_BLK_T_IN
            };

            (*blk_request).data.data = buffer;
            (*blk_request).header.reserved = 0;
            (*blk_request).status.status = 111;
            (*blk_request).watcher = watcher;

            let desc = Descriptor {addr: buffer as u64,
                                len: size,
                            flags: io:: IO_DESC_F_NEXT | if !write {
                                io::IO_DESC_F_WRITE
                            } else {
                                0
                            },
                        next: 0, };
            let _data_idx = fill_next_descriptor(bdev, desc);
            let desc = Descriptor {addr: &(*blk_request).status as *const Status as u64,
                                len: size_of::<Status>() as u32,
                                flags: io::IO_DESC_F_WRITE,
                                next: 0, };
            let _status_idx = fill_next_descriptor(bdev, desc);
            (*bdev.queue).avail.ring[(*bdev.queue).avail.idx as usize % io::IO_RING_SIZE] = head_idx;
            (*bdev.queue).avail.idx = (*bdev.queue).avail.idx.wrapping_add(1);
            bdev.dev.add(MmioOffsets::QueueNotify.scale32()).write_volatile(0);
            Ok(size)
        }
        else {
            Err(BlockErrors::BlockDeviceNotFound)
        }
    }
}

pub fn read(dev: usize,
            buffer: *mut u8,
            size: u32,
            offset: u64) -> Result<u32, BlockErrors> {
                block_op(dev, buffer, size, offset, false = 0)
            }

pub fn write(dev: usize,
            buffer: *mut u8,
            size: u32,
            offset: u64) -> Result<u32, BlockErrors> {
                block_op(dev, buffer, size, offset, true, 0)
            }

pub fn pending(bd: &mut BlockDevice) {
    unsafe {
        let ref queue = *bd.queue;
        while bd.ack_used_idx != queue.used.idx {
            let ref elem = queue.used.ring[bd.ack_used_idx as usize % IO_RING_SIZE];
            bd.ack_used_idx = bd.ack_used_idx.wrapping_add(1);
            let rq = queue.desc[elem.id as usize].addr as *const Request;
            let pid_of_watcher = (*rq).watcher;
            if pid_of_watcher > 0 {
                set_running(pid_of_watcher);
                let proc = get_by_pid(pid_of_watcher);
                (*(*proc).frame).regs[10] = (*rq).status.status as usize;
            }
            kfree(rq as *mut u8);
        }
    }
}

pub fn handle_interrupt(idx: usize) {
    unsafe {
        if let Some(bdev) = BLOCK_DEVICES[idx].as_mut() {
            pending(bdev);
        } else {
            println!("Invalid block device for interrupt {}", idx + 1);
        }
    }
}

// kernel (?!)
struct ProcArgs {
    pub pid: u16,
    pub dev: usize,
    pub buffer: *mut u8,
    pub size: u32,
    pub offset: u64,
}

fn read_proc(args_addr: usize) {
    let args = unsafe {Box::from_raw(args_addr as *mut ProcArgs)};
    let _ = block_op(args.dev, args.buffer, args.size, args.offset, false, args.pid,);
}

fn process_read(pid: u16, dev: usize, buffer: *mut u8, size: u32, offset: u64) {
    let args = ProcArgs {
        pid, dev, buffer, size, offset,
    };
    let boxed_args = Box::new(args);
    set_waiting(pid);
    let _ = add_kernel_process_args(read_proc, Box::into_raw(boxed_args) as usize,);
}

fn write_proc(args_addr: usize) {
    let args = unsafe {Box::from_raw(args_addr as *mut ProcArgs)};
    let _ = block_op(args.dev, args.buffer, args.size, args.offset, true, args.pid);
}

pub fn process_write(pid: u16, dev: usize, buffer: *mut u8, size: u32, offset: u64) {
    let args = ProcArgs {
        pid, dev, buffer, size, offset,
    };
    let boxed_args = Box::new(args);
    set_waiting(pid);
    let _ = add_kernel_process_args(write_proc, Box::into_raw(boxed_args) as usize,);
}