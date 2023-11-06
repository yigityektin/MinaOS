use core::{convert::TryInto, fmt::{Error, Write}};
use crate::console::push_stdin;

pub struct Uart {
    base_address: usize,
}

impl Write for Uart {
    fn write_str(&mut self, out: &str) -> Result<(), Error> {
        for c in out.bytes() {
            self.put(c);
        }
        Ok(())
    }
}

impl Uart {
    pub fn new(base_address: usize) -> Self {
        Uart { base_address }
    }

    pub fn init(&mut self) {
        let ptr = self.base_address as *mut u8;
        unsafe {
            let lcr: u8 = (1 << 0) | (1 << 1);
            ptr.add(3). write_volatile(lcr);
            ptr.add(2).write_volatile(1 << 0);
            ptr.add(1).write_volatile(1 << 0);

            let divisor: u16 = 592;
            let divisor_least: u8 = (divisor & 0xff).try_into().unwrap();
            let divisor_most: u8 = (divisor >> 8).try_into().unwrap();

            ptr.add(3).write_volatile(lcr | 1 << 7);
            ptr.add(0).write_volatile(divisor_least);
            ptr.add(1).write_volatile(divisor_most);

            ptr.add(3).write_volatile(lcr);
        }
    }

    pub fn put(&mut self, c: u8) {
        let ptr = self.base_address as *mut u8;
        unsafe {
            ptr.add(0).write_volatile(c);
        }
    }

    pub fn get(&mut self) -> Option<u8> {
        let ptr = self.base_address as *mut u8;
        unsafe {
            if ptr.add(5).read_volatile() & 1 == 0 {
                None
            } else {
                Some(ptr.add(0).read_volatile())
            }
        }
    }
}

pub fn handle_interrupt() {
    let mut my_uart = Uart::new(0x1000_0000);

    if let Some(c) = my_uart.get() {
        push_stdin(c);
        match c {
            8 => {
                print!("{} {}", 8 as char, 8 as char);
            },
            10 | 13 => {
                println!();
            },
            _ => {
                print!("{}", c as char);
            },
        }
    }
}