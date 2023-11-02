.option norvc
.altmacro
.set NUM_GP_REGS, 32
.set REG_SIZE, 8

.macro save_gp i, basereg=t6
    sd x\i, ((\i)*REG_SIZE)(\basereg)
.endm

.macro load_gp i, basereg=t6
    ld x\i, ((\i)*REG_SIZE)(\basereg)
.endm

.macro save_fp i, basereg=t6
    fsd f\i, ((NUM_GP_REGS+(\i))*REG_SIZE)(\basereg)
.endm

.macro load_fp i, basereg=t6
    fld f\i, ((NUM_GP_REGS+(\i))*REG_SIZE)(\basereg)
.endm

.section .text
.global m_trap_vector
.align 4
m_trap_vector:
    .set i, 0
    .rept 31
        save_gp %i
        .set i, i+1
    .endr

    mv t5, t6
    csrr t6, mscratch
    save_gp 31, t5

    csrw mscratch, t5

    csrr t1, mstatus
    srli t0, t1, 13
    andi t0, t0, 3
    li t3, 3
    bne t0, t3, 1f

    .set i, 0
    .rept 32
        save_fp %i, t5
        .set i, i+1
    .endr

1:
    # trap.rs part

    csrr a0, mepc
    sd a0, 520(t5)
    csrr a1, mtval
    csrr a2, mcause
    csrr a3, mhartid
    csrr a4, mstatus
    csrr a5, mscratch
    la t0, KERNEL_STACK_END
    ld sp, 0(t0)
    call m_trap

    csrw mepc, a0
    csrr t6, mscratch
    csrr t1, mstatus
    srli t0, t1, 13
    andi t0, t0, 3
    li t3, 3
    bne t0, t3, 1f
    .set i, 0
    .rept 32
        load_fp %i
        .set i, i+1
    .endr

1:
    .set i, 1
    .rept 31
        load_gp %i
        .set i, i+1
    .endr
    mret

.global switch_to_user
switch_to_user:
    csrw mscratch, a0
        ld a1, 520(a0)
        ld a2, 512(a0)
        ld a3, 552(a0)
        li t0, 1 << 7 | 1 << 5 | 1 << 13

        slli a3, a3, 11
        or t0, t0, a3
        csrw mstatus, t0
        csrw mepc, a11
        csrw satp, a2
        li t1, 0xaaa
        csrw mie, t1
        la t2, m_trap_vector
        csrw mtvec, t2

        mv t6, a0
        .set i, 0
        .rept 32
            load_fp %i
            .set i, i+1
        .endr
1:
        .set i, 1
        .rept 31
            load_gp %i, t6
            .set i, i+1
        .endr
    mret

.global make_syscall
make_syscall:
    mv a7, a0
    mv a0, a1
    mv a1, a2
    mv a2, a3
    mv a3, a4
    mv a4, a5
    mv a5, a6
    ecall
    ret