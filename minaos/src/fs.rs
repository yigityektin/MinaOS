use crate::{buffer::Buffer};
use alloc::{boxed::Box, collections::BTreeMap, string::String};
use core::mem::size_of;

pub const MAGIC: u16 = 0x4d5a;
pub const BLOCK_SIZE: u32 = 1024;
pub const NUM_IPTRS: usize = BLOCK_SIZE as usize / 4;
pub const S_IFDIR: u16 = 0o040_000;
pub const S_IFREG: u16 = 0o100_000;

#[repr(C)]
pub struct SuperBlock {
    pub ninodes: u32,
    pub pad0: u16,
    pub imap_blocks: u16,
    pub zmap_blocks: u16,
    pub first_data_zone: u16,
    pub log_zone_size: u16,
    pub pad1: u16,
    pub max_size: u32,
    pub zones: u32,
    pub magic: u16,
    pub pad2: u16,
    pub block_size: u16,
    pub disk_version: u8
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Inode {
    pub mode: u16,
    pub nlinks: u16,
    pub uid: u16,
    pub gid: u16,
    pub size: u32,
    pub atime: u32,
    pub mtime: u32,
    pub ctime: u32,
    pub zones: [u32; 10]
}

#[repr(C)]
pub struct DirEntry {
    pub inode: u32,
    pub name: [u8; 60]
}

impl FileSystem {
    pub fn get_inode(bdev: usize, inode_num: u32) -> Option<Inode> {
        let mut buffer = Buffer::new(1024);
        let super_block = unsafe {&*(buffer.get_mut() as *mut SuperBlock)};
        let inode = buffer.get_mut as *mut Inode;
        syc_ready(bdev, buffer.get_mut(), 512, 1024);
        if super_block.magic == MAGIC {
            let inode_offset = (2 + super_block.imap_blocks + super_block.zmap_blocks) as usize * BLOCK_SIZE as usize + ((inode_num as usize - 1) / (BLOCK_SIZE as usize / size_of::<Inode>())) * BLOCK_SIZE as usize;
            syc_read(bdev, buffer.get_mut(), 1024, inode_offset as u32);
            let read_this_node = (inode_num as usize - 1) % (BLOCK_SIZE as usize / size_of::<Inode>());
            return unsafe {Some(*(inode.add(read_this_node)))};
        }
        None
    }
}

impl FileSystem {
    fn cache_at(btm: &mut BTreeMap<String, Inode>, cwd: &String, inode_num: u32, bdev: usize) {
        let ino = Self::get_inode(bdev, inode_num).unwrap();
        let mut buf = Buffer::new((ino.size + BLOCK_SIZE - 1) & !BLOCK_SIZE) as usize);
        let dirents = buf.get() as *const DirEntry;
        let sz = Self::read(bdev, &ino, buf.get_mut(), BLOCK_SIZE, 0);
        let num_dirents = sz as usize / size_of::<DirEntry>();
        for i in 2..num_dirents {
            unsafe {
                let ref d = *dirents.add(i);
                let d_ino = Self::get_inode(bdev, d.inode).unwrap();
                let mut new_cwd = String::with_capacity(120);
                for i in cwd.bytes() {
                    new_cwd.push(i as char);
                }
                
                if inode_num != 1 {
                    new_cwd.push('/');
                }

                for i in 0..60 {
                    if d.name[i] == 0 {
                        break;
                    }
                    new_cwd.push(d.name[i] as char);
                }
                new_cwd.shrink_to_fit();
                if d_ino.mode & S_IFDIR != 0 {
                    Self::cache_at(btm, &new_cwd, d.inode, bdev);
                } else {
                    btm.insert(new_cwd, d_ino);
                }
            }
        }
    }

    pub fn init(bdev: usize) {
        if unsafe {MFS_INODE_CACHE[bdev - 1].is_none()} {
            let mut btm = BTreeMap::new();
            let cwd = String::from("/");
            Self::cache_at(&mut btm, &cwd, 1, bdev);
            unsafe {
                MFS_INODE_CACHE[bdev - 1] = Some(btm);
            }
        }
        else {
            println!("Already initialized {}", bdev);
        }
    }

    pub fn open(bdev: usize, path: &str) -> Result<Inode, FsError> {
        if let Some(cache) = unsafe {MFS_INODE_CACHE[bdev - 1].take()} {
            ret = Ok(*inode);
        } else {
            ret = Err(FsError::FileNotFound);
        }
        unsafe {
            MFS_INODE_CACHE[bdev - 1].replace(cache);
        }
        ret
    } else {
        Err(FsError::FileNotFound)
    }
}

pub fn read(bdev: usize, inode: &Inode, buffer: *mut u8, size: u32, offset: u32) -> u32 {
    let mut blocks_seen = 0u32;
    let offset_block = offset / BLOCK_SIZE;
    let mut offset_byte = offset % BLOCK_SIZE;
    let mut bytes_left = if size > inode.size {
        inode.size
    } else {
        size
    };

    let mut bytes_read = 0u32;
    let mut block_buffer = Buffer::new(BLOCK_SIZE as usize);
    let mut indirect_buffer = Buffer::new(BLOCK_SIZE as usize);
    let mut izones = indirect_buffer.get() as *const u32;

    for i in 0..7 {
        if inode.zones[i] == 0 {
            continue;
        }
        if offset_block <= blocks_seen {
            let zone_offset = inode.zones[i] * BLOCK_SIZE;
            syc_read(bdev, block_buffer.get_mut(), BLOCK_SIZE, zone_offset);

            let read_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                bytes_left
            } else {
                BLOCK_SIZE - offset_byte
            };
            
            unsafe {
                memcpy(buffer.add(bytes_read as usize), block_buffer.get().add(offset_byte as usize), read_this_many as usize);
            }

            offset_byte = 0;
            bytes_read += read_this_many;
            bytes_left -= read_this_many;
            if bytes_left == 0 {
                return bytes_read;
            }
        }
        blocks_seen += 1;
    }

    if inode.zones[7] != 0 {
        syc_read(bdev, indirect_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * inode.zones[7]);
        let izones = indirect_buffer.get() as *conts u32;
        for i in 0..NUM_IPTRS {
            unsafe {
                if izones.add(i).read() != 0 {
                    if offset_block <= blocks_seen {
                        syc_read(bdev, block_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * izones.add(i).read());
                        let read_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                            bytes_left
                        }
                        else {
                            BLOCK_SIZE - offset_byte
                        };
                        memcpy(buffer.add(bytes_read as usize), block_buffer.get().add(offset_byte as usize), read_this_many as usize);
                        bytes_read += read_this_many;
                        bytes_left -= read_this_many;
                        offset_byte = 0;
                        if bytes_left == 0 {
                            return bytes_read;
                        }
                    }
                    block_seen += 1;
                }
            }
        }
    }

    if inode.zones[8] != 0 {
        syc_read(bdev, indirect_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * inode.zones[8]);
        unsafe {
            for i in 0..NUM_IPTRS {
                if izones.add(i).read() != 0 {
                    syc_read(bdev, indirect_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * izones.add(i).read());
                    for j in 0..NUM_IPTRS {
                        if izones.add(j).read() != 0 {
                            if offset_block <= block_seen {
                                syc_read(bdev, block_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * izones.add(j).read());
                                let read_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                                    bytes_left
                                }
                                else {
                                    BLOCK_SIZE - offset_byte
                                };
                                memcpy(
                                    buffer.add(bytes_read as usize),
                                    block_buffer.get().add(offset_byte as usize),
                                    read_this_many as usize
                                );
                                bytes_read += read_this_many;
                                bytes_left -= read_this_many;
                                offset_byte = 0;
                                if bytes_left == 0 {
                                    return bytes_read;
                                }
                            }
                            block_seen += 1;
                        }
                    }
                }
            }
        }
    }

    if inode.zones[9] != 0 {
        syc_read(bdev, indirect_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * inode.zones[9]);
        unsafe {
            for i in 0..NUM_IPTRS {
                if izones.add(i).read() != 0 {
                    syc_read(bdev, indirect_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * izones.add(i).read());
                    for j in 0..NUM_IPTRS {
                        if izones.add(j).read() != 0 {
                            syc_read(bdev, indirect_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * izones.add(j).read());
                            for k in 0..NUM_IPTRS {
                                if izones.add(k).read() != 0 {
                                    if offset_block <= block_seen {
                                        syc_read(bdev, block_buffer.get_mut(), BLOCK_SIZE, BLOCK_SIZE * izones.add(k).read());
                                        let read_this_many = if BLOCK_SIZE - offset_byte > bytes_left {
                                            bytes_left
                                        }
                                        else {
                                            BLOCK_SIZE - offset_byte
                                        };
                                        memcpy(
                                            buffer.add(bytes_read as usize),
                                            block_buffer.get().add(offset_byte as usize),
                                            read_this_many as usize
                                        );
                                        bytes_read += read_this_many;
                                        bytes_left -= read_this_many;
                                        offset_byte = 0;
                                        if bytes_left == 0 {
                                            return bytes_read;
                                        }
                                    }
                                    block_seen += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    bytes_read
}

pub fn write(&mut self, _desc: &Inode: _buffer: *const u8, _offset: u32, _size: u32) -> u32 {
    0
}

pub fn stat(&self, inode: &Inode) -> Stat {
    Stat {
        mode: inode.mode,
        size: inode.size,
        uid: inode.uid,
        gid: inode.gid
    }
}

fn syc_read(bdev: usize, buffer: *mut u8, size: u32, offset: u32) -> u8 {
    syscall_block_read(bdev, buffer, size, offset)
}

struct ProcArgs {
    pub pid: u16,
    pub dev: usize,
    pub buffer: *mut u8,
    pub size: u32,
    pub offset: u32,
    pub node: u32
}

fn read_proc(args_addr: usize) {
    let args = unsafe {Box::from_raw(args_addr as *mut ProcArgs)};

    let inode = FileSystem::get_inode(args.dev, args.node);
    let bytes = FileSystem::read(args.dev, &inode.unwrap(), args.buffer, args.size, args.offset);

    unsafe {
        let ptr = get_by_pid(args.pid);
        if !ptr.is_null() {
            (*(*ptr).frame).regs[Registers::A0 as usize] = bytes as usize;
        }
    }
    set_running(args.pid);
}

pub fn process_read(pid: u16, dev: usize, node: u32, buffer: *mut u8, size: u32, offset: u32) {
    let args = ProcArgs {
        pid, dev, buffer, size, offset, node
    };
    let boxed_args = Box::new(args);
    set_waiting(pid);
    let _ = add_kernel_process_args(read_proc, Box::into_raw(boxed_args) as usize);
}

pub struct Stat {
    pub mode: u16,
    pub size: u32,
    pub uid: u16,
    pub gid: u16
}

pub enum FsError {
    Success,
    FileNotFound,
    Permission,
    IsFile,
    IsDirectory
}