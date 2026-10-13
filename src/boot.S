.section .text.boot
.global _start

_start:
    ldr x0, =__stack_top
    mov sp, x0

    // Zero BSS
    ldr x1, =__bss_start
    ldr x2, =__bss_end
1:
    cmp x1, x2
    b.ge 2f
    str xzr, [x1], #8
    b 1b
2:
    ldr x0, =vectors
    msr vbar_el1, x0
    isb

    mrs x0, cpacr_el1
    orr x0, x0, #(3 << 20)
    msr cpacr_el1, x0
    isb

    bl kmain

3:
    wfe
    b 3b

// -----------------------------------------------------------------
// Vectors
// 16 entries, 0x80 bytes apart, 0x800 bytes total.
// Group 1 (0x000): Current EL, SP0
// Group 2 (0x200): Current EL, SPx
// Group 3 (0x400): Lower EL, AArch64   <- SVC from EL0 lands here
// Group 4 (0x600): Lower EL, AArch32
// -----------------------------------------------------------------
.align 11
.global vectors
vectors:
    b exc_sync
    .space 0x80 - 4
    b exc_irq
    .space 0x80 - 4
    b exc_fiq
    .space 0x80 - 4
    b exc_serr
    .space 0x80 - 4

    b exc_sync
    .space 0x80 - 4
    b exc_irq
    .space 0x80 - 4
    b exc_fiq
    .space 0x80 - 4
    b exc_serr
    .space 0x80 - 4

    // Group 3: Lower EL, AArch64
    b exc_svc
    .space 0x80 - 4
    b exc_irq
    .space 0x80 - 4
    b exc_fiq
    .space 0x80 - 4
    b exc_serr
    .space 0x80 - 4

    b exc_sync
    .space 0x80 - 4
    b exc_irq
    .space 0x80 - 4
    b exc_fiq
    .space 0x80 - 4
    b exc_serr
    .space 0x80 - 4

// -----------------------------------------------------------------
// Exception handlers
// -----------------------------------------------------------------

exc_sync:
    mrs x0, esr_el1
    mrs x1, elr_el1
    mrs x2, far_el1
    bl rust_exception_handler
    b hang

// IRQ handler: switch context via Context* in TPIDR_EL1
exc_irq:
    sub sp, sp, #0x20
    str x9,  [sp, #0x00]
    str x10, [sp, #0x08]

    mrs x9, tpidr_el1

    // Save x0..x8
    stp x0,  x1,  [x9, #0x00]
    stp x2,  x3,  [x9, #0x10]
    stp x4,  x5,  [x9, #0x20]
    stp x6,  x7,  [x9, #0x30]
    str x8,          [x9, #0x40]

    // Save original x9 and x10 from scratch
    ldr x0, [sp, #0x00]
    str x0, [x9, #0x48]
    ldr x0, [sp, #0x08]
    str x0, [x9, #0x50]

    // Save x11..x30
    stp x11, x12, [x9, #0x58]
    stp x13, x14, [x9, #0x68]
    stp x15, x16, [x9, #0x78]
    stp x17, x18, [x9, #0x88]
    stp x19, x20, [x9, #0x98]
    stp x21, x22, [x9, #0xA8]
    stp x23, x24, [x9, #0xB8]
    stp x25, x26, [x9, #0xC8]
    stp x27, x28, [x9, #0xD8]
    stp x29, x30, [x9, #0xE8]

    // Save SP, ELR, SPSR
    add x0, sp, #0x20
    str x0, [x9, #0xF8]
    mrs x0, elr_el1
    str x0, [x9, #0x100]
    mrs x0, spsr_el1
    str x0, [x9, #0x108]

    // Call Rust IRQ handler
    mov x0, x9
    bl rust_irq_handler

    // x0 = next context pointer
    msr tpidr_el1, x0
    mov x19, x0

    // Restore SP/ELR/SPSR for next thread
    ldr x1, [x19, #0xF8]
    mov sp, x1
    ldr x1, [x19, #0x100]
    msr elr_el1, x1
    ldr x1, [x19, #0x108]
    msr spsr_el1, x1

    // Restore x0..x8
    ldp x0,  x1,  [x19, #0x00]
    ldp x2,  x3,  [x19, #0x10]
    ldp x4,  x5,  [x19, #0x20]
    ldp x6,  x7,  [x19, #0x30]
    ldp x8,  x9,  [x19, #0x40]

    ldp x10, x11, [x19, #0x50]
    ldp x12, x13, [x19, #0x60]
    ldp x14, x15, [x19, #0x70]
    ldr x16,       [x19, #0x80]
    ldp x17, x18,  [x19, #0x88]

    ldp x20, x21, [x19, #0xA0]
    ldp x22, x23, [x19, #0xB0]
    ldp x24, x25, [x19, #0xC0]
    ldp x26, x27, [x19, #0xD0]
    ldp x28, x29, [x19, #0xE0]
    ldr x30,       [x19, #0xF0]

    // Restore x19 last
    ldr x19, [x19, #0x98]
    eret

// -----------------------------------------------------------------
// SVC handler — called from EL0 via `svc #n`
// The CPU has already saved ELR_EL1, SPSR_EL1, and set ESR_EL1.
// We save the user's registers into the Context, call the Rust
// dispatcher, restore, and eret back to EL0.
// -----------------------------------------------------------------
exc_svc:
    // Save x9/x10 to scratch
    sub sp, sp, #0x20
    str x9,  [sp, #0x00]
    str x10, [sp, #0x08]

    // x9 = current Context*
    mrs x9, tpidr_el1

    // Save x0..x8
    stp x0,  x1,  [x9, #0x00]
    stp x2,  x3,  [x9, #0x10]
    stp x4,  x5,  [x9, #0x20]
    stp x6,  x7,  [x9, #0x30]
    str x8,          [x9, #0x40]

    // Recover original x9/x10
    ldr x0, [sp, #0x00]
    str x0, [x9, #0x48]
    ldr x0, [sp, #0x08]
    str x0, [x9, #0x50]

    // Save x11..x30
    stp x11, x12, [x9, #0x58]
    stp x13, x14, [x9, #0x68]
    stp x15, x16, [x9, #0x78]
    stp x17, x18, [x9, #0x88]
    stp x19, x20, [x9, #0x98]
    stp x21, x22, [x9, #0xA8]
    stp x23, x24, [x9, #0xB8]
    stp x25, x26, [x9, #0xC8]
    stp x27, x28, [x9, #0xD8]
    stp x29, x30, [x9, #0xE8]

    // Save SP_EL0 (user stack pointer) at 0x110
    mrs x0, sp_el0
    str x0, [x9, #0x110]

    // Save ELR_EL1 (user PC) and SPSR_EL1 (user PSTATE)
    mrs x0, elr_el1
    str x0, [x9, #0x100]
    mrs x0, spsr_el1
    str x0, [x9, #0x108]

    // Restore our stack pointer
    add sp, sp, #0x20

    // Read SVC immediate from ESR_EL1[15:0]
    mrs x0, esr_el1
    and x0, x0, #0xFFFF

    // Call rust_svc_handler(svc_imm, context_ptr)
    mov x1, x9
    bl rust_svc_handler

    // x0 = return value to store in user's x0
    mrs x9, tpidr_el1
    str x0, [x9, #0x00]

    // Restore user SP_EL0
    ldr x0, [x9, #0x110]
    msr sp_el0, x0

    // Restore ELR_EL1 / SPSR_EL1
    ldr x0, [x9, #0x100]
    msr elr_el1, x0
    ldr x0, [x9, #0x108]
    msr spsr_el1, x0

    // Restore user registers (x19 as base, restored last)
    mov x19, x9
    ldp x0,  x1,  [x19, #0x00]
    ldp x2,  x3,  [x19, #0x10]
    ldp x4,  x5,  [x19, #0x20]
    ldp x6,  x7,  [x19, #0x30]
    ldr x8,       [x19, #0x40]
    ldr x9,       [x19, #0x48]
    ldr x10,      [x19, #0x50]
    ldp x11, x12, [x19, #0x58]
    ldp x13, x14, [x19, #0x68]
    ldp x15, x16, [x19, #0x78]
    ldp x17, x18, [x19, #0x88]
    ldr x20,      [x19, #0xA0]
    ldp x21, x22, [x19, #0xA8]
    ldp x23, x24, [x19, #0xB8]
    ldp x25, x26, [x19, #0xC8]
    ldp x27, x28, [x19, #0xD8]
    ldp x29, x30, [x19, #0xE8]
    ldr x19, [x19, #0x98]
    eret

exc_fiq:
    b hang
exc_serr:
    b hang

hang:
    wfe
    b hang

// -----------------------------------------------------------------
// start_first: enter the first thread by eret-ing into its context.
// x0 = Context*
// -----------------------------------------------------------------
.global start_first
start_first:
    msr tpidr_el1, x0
    mov x19, x0

    ldr x1, [x19, #0xF8]
    mov sp, x1
    ldr x1, [x19, #0x100]
    msr elr_el1, x1
    ldr x1, [x19, #0x108]
    msr spsr_el1, x1

    ldp x0,  x1,  [x19, #0x00]
    ldp x2,  x3,  [x19, #0x10]
    ldp x4,  x5,  [x19, #0x20]
    ldp x6,  x7,  [x19, #0x30]
    ldp x8,  x9,  [x19, #0x40]
    ldp x10, x11, [x19, #0x50]
    ldp x12, x13, [x19, #0x60]
    ldp x14, x15, [x19, #0x70]
    ldr x16,       [x19, #0x80]
    ldp x17, x18,  [x19, #0x88]
    ldp x20, x21, [x19, #0xA0]
    ldp x22, x23, [x19, #0xB0]
    ldp x24, x25, [x19, #0xC0]
    ldp x26, x27, [x19, #0xD0]
    ldp x28, x29, [x19, #0xE0]
    ldr x30,       [x19, #0xF0]
    ldr x19, [x19, #0x98]
    eret

// -----------------------------------------------------------------
// enable_mmu(ttbr0: u64)
// -----------------------------------------------------------------
.global enable_mmu
enable_mmu:
    mov x1, #0xFF
    mov x2, #0x04
    lsl x2, x2, #8
    orr x1, x1, x2
    msr mair_el1, x1

    mov x1, #16
    mov x2, #(0b11)
    lsl x2, x2, #12
    orr x1, x1, x2
    mov x2, #(0b01)
    lsl x2, x2, #10
    orr x1, x1, x2
    mov x2, #(0b01)
    lsl x2, x2, #8
    orr x1, x1, x2
    mov x2, #(1)
    lsl x2, x2, #23
    orr x1, x1, x2
    mov x2, #(0b010)
    lsl x2, x2, #32
    orr x1, x1, x2
    msr tcr_el1, x1

    msr ttbr0_el1, x0

    dsb sy
    isb
    tlbi vmalle1
    dsb ish
    isb

    mrs x1, sctlr_el1
    orr x1, x1, #1
    bic x1, x1, #(1 << 2)
    bic x1, x1, #(1 << 12)
    msr sctlr_el1, x1
    isb
    ret
