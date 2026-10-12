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
    // We need scratch registers. Use stack.
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

    // Save original x9 and x10 from scratch
    ldr x0, [sp, #0x00]
    str x0, [x9, #0x48]   // x9 slot
    ldr x0, [sp, #0x08]
    str x0, [x9, #0x50]   // x10 slot

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

    // Save original SP, ELR, SPSR
    add x0, sp, #0x20
    str x0, [x9, #0xF8]
    mrs x0, elr_el1
    str x0, [x9, #0x100]
    mrs x0, spsr_el1
    str x0, [x9, #0x108]

    // Call Rust: x0 = current ctx, returns x0 = next ctx
    mov x0, x9
    bl rust_irq_handler

    // Update TPIDR_EL1 to next
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
