use crate::{cpu::{TrapFrame, CONTEXT_SWITCH_TIME},
    plic,
    process::delete_process,
    rust_switch_to_user,
    sched::schedule,
    syscall::do_syscall};

#[no_mangle]

extern "C" fn m_trap(epc: usize,
                    tval: usize,
                    hart: usize,
                    _status: usize,
                    frame: *mut TrapFrame)
                    -> usize
{
    let is_async = {
        if cause >> 63 & 1 == 1 {
            true
        } else {
            false
        }
    };

    let cause_num = cause & 0xfff;
    let mut return_pc = epc;
    if is_async {
        match cause_num {
            3 => {
                println!("Machine software interrupt CPU #{}", hart);
            }
            7 => {
                let new_frame = schedule();
                schedule_next_context_switch(1);
                if new_frame != 0 {
                    rust_switch_to_user(new_frame);
                }
            }
            11 => {
                plic::handle_interrupt();
            }
            _ => {
                panic!("Unhandled async trap CPU#{} -> {}\n", hart, cause_num);
            }
        }
    }
    else {
        match cause_num {
            2 => unsafe {
                println!("Illegal instruction CPU#{} -> 0x{:08x}: 0x{:08x}\n", hart, epc, tval);
                
                delete_process((*frame).pid as u16);
                let frame = schedule();
                schedule_next_context_switch(1),
                rust_switch_to_user(frame);
            }
            3 => {
                println!("Breakpoint\n\n");
                return_pc += 2;
            }
            7 => unsafe {
                println!("Error with pid {}, at PC 0x{:08x}, mepc 0x{:08x}", (*frame).pid, (*frame).pc, epc);
             
                delete_process((*frame).pid as u16); 
                let frame = schedule();
                schedule_next_context_switch(1);
                rust_switch_to_user(frame);
            }
            8 | 9 | 11 => unsafe {
                do_syscall(return_pc, frame);
                let frame = schedule();
                schedule_next_context_switch(1);
                rust_switch_to_user(frame);
            }
            12 => unsafe {
                println!("Instruction page fault CPU#{} -> 0x{:08x}: 0x{:08x}", hart, epc, tval);

                delete_process((*frame).pid as u16);
                let frame = schedule();
                schedule_next_context_switch(1);
                rust_switch_to_user(frame);
            }
            13 => unsafe {
                println!("Load page fault CPU#{} -> 0x{:08x}: 0x{:08x}", hart, epc, tval);

                delete_process((*frame).pid as u16);
                let frame = schedule();
                schedule_next_context_switch(1);
                rust_switch_to_user(frame);
            }
            15 => unsafe {
                println!("Store page fault CPU#{} -> 0x{:08x}: 0x{:08x}", hart, epc, tval);

                delete_process((*frame).pid as u16);
                let frame = schedule();
                schedule_next_context_switch(1);
                rust_switch_to_user(frame);
            }
            _ => {
                panic!("Unhandled sync trap {}. CPU#{} -> 0x{:08x}: 0x{:08x}\n", cause_num, hart, epc, tval);
            }
        }
    };
    return_pc
}

pub const MMIO_MTIMECMP: *mut u64 = 0x0200_4000usize as *mut u64;
pub const MMIO_MTIME: *const u64 = 0x0200_BFF8 as *const u64;

pub fn schedule_next_context_switch(qm: u16) {
    unsafe {
        MMIO_MTIMECMP.write_volatile(MMIO_MTIME.read_volatile().wrapping_add(CONTEXT_SWITCH_TIME * qm as u64));
    }
}