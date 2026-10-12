#![no_std]
#![no_main]

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ============================================================
// UART
// ============================================================

const UART0_BASE: usize = 0x0900_0000;
const UART0_DR:   usize = UART0_BASE + 0x00;
const UART0_FR:   usize = UART0_BASE + 0x18;
const UART0_TXFF: u32   = 1 << 5;
const UART0_RXFE: u32   = 1 << 4;

pub struct Uart;

impl Uart {
    fn putc(&self, c: u8) {
        unsafe {
            while (core::ptr::read_volatile(UART0_FR as *const u32) & UART0_TXFF) != 0 {}
            core::ptr::write_volatile(UART0_DR as *mut u32, c as u32);
        }
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' { self.putc(b'\r'); }
            self.putc(b);
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{ use core::fmt::Write; let _ = write!($crate::Uart, $($arg)*); }};
}

#[macro_export]
macro_rules! println {
    () => {{ use core::fmt::Write; let _ = write!($crate::Uart, "\n"); }};
    ($($arg:tt)*) => {{ use core::fmt::Write; let _ = write!($crate::Uart, $($arg)*); let _ = write!($crate::Uart, "\n"); }};
}

fn uart_getc() -> u8 {
    unsafe {
        while (core::ptr::read_volatile(UART0_FR as *const u32) & UART0_RXFE) != 0 {}
        (core::ptr::read_volatile(UART0_DR as *const u32) & 0xFF) as u8
    }
}

// ============================================================
// Helpers
// ============================================================

fn clear_screen() { print!("\x1b[2J\x1b[H"); }

fn read_cntfrq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v, options(nostack, nomem)); }
    v
}

fn read_cntpct() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) v, options(nostack, nomem)); }
    v
}

fn current_el() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, currentel", out(reg) v, options(nostack, nomem)); }
    (v >> 2) & 0x3
}

fn read_sp() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mov {}, sp", out(reg) v, options(nostack, nomem)); }
    v
}

fn read_elr() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, elr_el1", out(reg) v, options(nostack, nomem)); }
    v
}

fn read_spsr() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, spsr_el1", out(reg) v, options(nostack, nomem)); }
    v
}

fn sleep_ms(ms: u64) {
    let freq = read_cntfrq();
    let ticks = (freq / 1000) * ms;
    let start = read_cntpct();
    while read_cntpct().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

// ============================================================
// Memory primitives
// ============================================================

#[no_mangle]
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        *dest.add(i) = *src.add(i);
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memset(dest: *mut u8, c: i32, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        *dest.add(i) = c as u8;
        i += 1;
    }
    dest
}

// ============================================================
// MMIO + GIC + Timer
// ============================================================

fn mmio_write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val); }
}
fn mmio_read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

const GICD_BASE: usize = 0x0800_0000;
const GICC_BASE: usize = 0x0801_0000;

const GICD_CTLR:       usize = GICD_BASE + 0x000;
const GICD_ISENABLER0: usize = GICD_BASE + 0x100;
const GICD_IPRIORITYR: usize = GICD_BASE + 0x400;
const GICD_ITARGETSR:  usize = GICD_BASE + 0x800;

const GICC_CTLR: usize = GICC_BASE + 0x000;
const GICC_PMR:  usize = GICC_BASE + 0x004;
const GICC_IAR:  usize = GICC_BASE + 0x00C;
const GICC_EOIR: usize = GICC_BASE + 0x010;

const IRQ_CNTPNS: u32 = 30;

static TICKS: AtomicU64 = AtomicU64::new(0);

fn gic_init() {
    mmio_write32(GICD_CTLR, 1);
    mmio_write32(GICC_PMR, 0xFF);
    mmio_write32(GICC_CTLR, 1);
    let mask = 1u32 << IRQ_CNTPNS;
    mmio_write32(GICD_ISENABLER0, mask);
    unsafe {
        let pri = (GICD_IPRIORITYR + IRQ_CNTPNS as usize) as *mut u8;
        core::ptr::write_volatile(pri, 0x80);
        let tgt = (GICD_ITARGETSR + IRQ_CNTPNS as usize) as *mut u8;
        core::ptr::write_volatile(tgt, 0x01);
    }
}

fn timer_init() {
    let freq = read_cntfrq();
    let tval = freq / 10;
    unsafe {
        core::arch::asm!(
            "msr cntp_tval_el0, {tval}",
            "mov x0, #1",
            "msr cntp_ctl_el0, x0",
            tval = in(reg) tval, out("x0") _, options(nostack, nomem)
        );
    }
}

fn timer_rearm() {
    let freq = read_cntfrq();
    let tval = freq / 10;
    unsafe {
        core::arch::asm!("msr cntp_tval_el0, {tval}", tval = in(reg) tval, options(nostack, nomem));
    }
}

fn enable_irqs() {
    unsafe { core::arch::asm!("msr daifclr, #2", options(nostack, nomem)); }
}

// ============================================================
// Context / scheduler
// ============================================================

#[repr(C)]
pub struct Context {
    pub x: [u64; 31],
    pub sp: u64,
    pub elr: u64,
    pub spsr: u64,
}

const STACK_SIZE: usize = 16 * 1024;
const MAX_THREADS: usize = 8;

#[repr(align(16))]
struct Stack([u8; STACK_SIZE]);

static STACK_A: Stack = Stack([0; STACK_SIZE]);
static STACK_B: Stack = Stack([0; STACK_SIZE]);
static STACK_SHELL: Stack = Stack([0; STACK_SIZE]);
static STACK_1: Stack = Stack([0; STACK_SIZE]);
static STACK_2: Stack = Stack([0; STACK_SIZE]);
static STACK_3: Stack = Stack([0; STACK_SIZE]);
static STACK_4: Stack = Stack([0; STACK_SIZE]);
static STACK_5: Stack = Stack([0; STACK_SIZE]);

fn spawn_stack_base(idx: usize) -> Option<usize> {
    match idx {
        0 => Some(STACK_1.0.as_ptr() as usize),
        1 => Some(STACK_2.0.as_ptr() as usize),
        2 => Some(STACK_3.0.as_ptr() as usize),
        3 => Some(STACK_4.0.as_ptr() as usize),
        4 => Some(STACK_5.0.as_ptr() as usize),
        _ => None,
    }
}

static mut CTX: [Context; MAX_THREADS] = [
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5 },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static NUM_THREADS: AtomicUsize = AtomicUsize::new(3);
static NEXT_SPAWN_SLOT: AtomicUsize = AtomicUsize::new(0);

extern "C" {
    fn start_first(ctx: *const Context) -> !;
}

extern "C" fn thread_a() -> ! { loop { core::hint::spin_loop(); } }
extern "C" fn thread_b() -> ! { loop { core::hint::spin_loop(); } }

extern "C" fn spawned_thread_entry() -> ! {
    println!("[spawned thread running]");
    loop { core::hint::spin_loop(); }
}

pub fn sched_init() {
    unsafe {
        CTX[0].sp = STACK_A.0.as_ptr().add(STACK_SIZE) as u64;
        CTX[0].elr = thread_a as *const () as usize as u64;
        CTX[0].spsr = 0x5;

        CTX[1].sp = STACK_B.0.as_ptr().add(STACK_SIZE) as u64;
        CTX[1].elr = thread_b as *const () as usize as u64;
        CTX[1].spsr = 0x5;

        CTX[2].sp = STACK_SHELL.0.as_ptr().add(STACK_SIZE) as u64;
        CTX[2].elr = shell_thread as *const () as usize as u64;
        CTX[2].spsr = 0x5;
    }
}

fn spawn_thread() -> Option<usize> {
    let slot = NEXT_SPAWN_SLOT.fetch_add(1, Ordering::Relaxed);
    let stack_base = spawn_stack_base(slot)?;

    let n = NUM_THREADS.load(Ordering::Relaxed);
    if n >= MAX_THREADS {
        return None;
    }
    let idx = n;
    unsafe {
        CTX[idx].sp = stack_base as u64 + STACK_SIZE as u64;
        CTX[idx].elr = spawned_thread_entry as *const () as usize as u64;
        CTX[idx].spsr = 0x5;
    }
    NUM_THREADS.store(n + 1, Ordering::Relaxed);
    Some(idx)
}

// ============================================================
// Shell
// ============================================================

const MAX_LINE: usize = 128;
const HISTORY_SIZE: usize = 8;

static mut HISTORY: [[u8; MAX_LINE]; HISTORY_SIZE] = [[0; MAX_LINE]; HISTORY_SIZE];
static HISTORY_COUNT: AtomicUsize = AtomicUsize::new(0);

fn history_push(line: &str) {
    let idx = HISTORY_COUNT.load(Ordering::Relaxed) % HISTORY_SIZE;
    unsafe {
        let slot = &mut HISTORY[idx];
        let bytes = line.as_bytes();
        let n = bytes.len().min(MAX_LINE - 1);
        for i in 0..n {
            slot[i] = bytes[i];
        }
        slot[n] = 0;
    }
    HISTORY_COUNT.fetch_add(1, Ordering::Relaxed);
}

fn shell_prompt() { print!("adityaos> "); }

fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(s, 16).ok()
}

fn parse_u64(s: &str) -> Option<u64> {
    s.parse::<u64>().ok()
}

fn dump_hex_ascii(addr: u64, len: usize) {
    println!("Dump at {:#x}, {} bytes:", addr, len);
    let mut offset = 0;
    while offset < len {
        print!("{:#010x}: ", addr + offset as u64);

        let mut ascii = [b'.'; 16];
        let mut i = 0;
        while i < 16 && offset + i < len {
            let byte = unsafe { core::ptr::read_volatile((addr + (offset + i) as u64) as *const u8) };
            print!("{:02x} ", byte);
            ascii[i] = if byte >= 0x20 && byte <= 0x7E { byte } else { b'.' };
            i += 1;
        }
        while i < 16 {
            print!("   ");
            i += 1;
        }
        print!(" |");
        for j in 0..16 {
            if offset + j < len {
                Uart.putc(ascii[j]);
            } else {
                Uart.putc(b' ');
            }
        }
        println!("|");
        offset += 16;
    }
}

fn handle_command(line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() { return; }

    history_push(trimmed);

    let mut parts = trimmed.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("").trim();

    match cmd {
        "help" => {
            println!("AdityaOS shell commands:");
            println!("  help         - show this message");
            println!("  clear        - clear screen");
            println!("  ticks        - show timer tick count");
            println!("  uptime       - show seconds since boot");
            println!("  time         - same as uptime (counter-based)");
            println!("  threads      - show thread/scheduler info");
            println!("  el           - show current exception level");
            println!("  regs         - dump CPU state");
            println!("  echo <text>  - print text back");
            println!("  sleep <ms>   - sleep for N milliseconds");
            println!("  mem <addr>   - dump 128 bytes at hex addr");
            println!("  history      - show command history");
            println!("  spawn        - create a new thread");
            println!("  panic        - trigger a kernel panic");
            println!("  reboot       - reset the system (PSCI)");
        }
        "clear" => clear_screen(),
        "ticks" => println!("Ticks: {}", TICKS.load(Ordering::Relaxed)),
        "uptime" | "time" => {
            let secs = read_cntpct() / read_cntfrq();
            println!("Uptime: {} seconds", secs);
        }
        "threads" => {
            println!("Threads: {}", NUM_THREADS.load(Ordering::Relaxed));
            println!("Current: {}", CURRENT.load(Ordering::Relaxed));
            println!("Switches: {}", SWITCHES.load(Ordering::Relaxed));
        }
        "el" => println!("CurrentEL: EL{}", current_el()),
        "regs" => {
            println!("SP:        {:#018x}", read_sp());
            println!("ELR_EL1:   {:#018x}", read_elr());
            println!("SPSR_EL1:  {:#018x}", read_spsr());
            println!("CurrentEL: EL{}", current_el());
        }
        "echo" => println!("{}", args),
        "sleep" => {
            if let Some(ms) = parse_u64(args) {
                println!("Sleeping {} ms...", ms);
                sleep_ms(ms);
                println!("Awake.");
            } else {
                println!("Usage: sleep <ms>");
            }
        }
        "mem" => {
            if let Some(addr) = parse_hex(args) {
                dump_hex_ascii(addr, 128);
            } else {
                println!("Usage: mem <hex address>  e.g. mem 0x40080000");
            }
        }
        "history" => {
            let total = HISTORY_COUNT.load(Ordering::Relaxed);
            let count = total.min(HISTORY_SIZE);
            println!("Last {} commands:", count);
            let start = if total > HISTORY_SIZE { total - HISTORY_SIZE } else { 0 };
            for i in 0..count {
                let idx = (start + i) % HISTORY_SIZE;
                let s = unsafe {
                    let bytes = &HISTORY[idx];
                    let mut end = 0;
                    while end < MAX_LINE && bytes[end] != 0 { end += 1; }
                    core::str::from_utf8_unchecked(&bytes[..end])
                };
                println!("  {}", s);
            }
        }
        "spawn" => {
            match spawn_thread() {
                Some(idx) => {
                    println!("Spawned thread index {}", idx);
                    println!("Type 'threads' to see the count.");
                }
                None => println!("No free thread slots."),
            }
        }
        "panic" => panic!("user-triggered panic"),
        "reboot" => {
            println!("Rebooting via PSCI...");
            sleep_ms(200);
            unsafe {
                core::arch::asm!(
                    "movz x0, #0x0009",
                    "movk x0, #0x8400, lsl #16",
                    "hvc #0",
                    out("x0") _,
                    options(nostack, nomem)
                );
            }
        }
        _ => {
            println!("Unknown command: '{}'", cmd);
            println!("Type 'help' for a list of commands.");
        }
    }
}

extern "C" fn shell_thread() -> ! {
    let mut buf = [0u8; MAX_LINE];
    let mut len: usize = 0;

    sleep_ms(500);

    println!();
    println!("=================================");
    println!("     AdityaOS Shell v1.0");
    println!("   Type 'help' for commands.");
    println!("=================================");
    println!();

    shell_prompt();

    loop {
        let c = uart_getc();

        match c {
            0x0D | 0x0A => {
                println!();
                if len < MAX_LINE { buf[len] = 0; }
                let line = unsafe { core::str::from_utf8_unchecked(&buf[..len]) };
                handle_command(line);
                len = 0;
                shell_prompt();
            }
            0x7F | 0x08 => {
                if len > 0 {
                    len -= 1;
                    print!("\x08 \x08");
                }
            }
            0x20..=0x7E => {
                if len < MAX_LINE - 1 {
                    buf[len] = c;
                    len += 1;
                    Uart.putc(c);
                }
            }
            _ => {}
        }
    }
}

// ============================================================
// Exception + IRQ
// ============================================================

#[no_mangle]
pub extern "C" fn rust_exception_handler(esr: u64, elr: u64, far: u64) {
    clear_screen();
    println!("=== AdityaOS Exception ===");
    println!("  ESR_EL1: {:#018x}", esr);
    println!("  ELR_EL1: {:#018x}", elr);
    println!("  FAR_EL1: {:#018x}", far);
    let ec = (esr >> 26) & 0x3F;
    println!("  EC: {:#x}", ec);
    println!("System halted.");
}

#[no_mangle]
pub extern "C" fn rust_irq_handler(current: *mut Context) -> *const Context {
    let iar = mmio_read32(GICC_IAR);
    let id = iar & 0x3FF;

    let mut next: *const Context = current;

    if id == IRQ_CNTPNS {
        let t = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        timer_rearm();

        if (t % 2) == 0 {
            SWITCHES.fetch_add(1, Ordering::Relaxed);
            let cur = CURRENT.load(Ordering::Relaxed);
            let n = NUM_THREADS.load(Ordering::Relaxed).max(1);
            let nxt = (cur + 1) % n;
            CURRENT.store(nxt, Ordering::Relaxed);
            unsafe {
                next = &CTX[nxt] as *const Context;
            }
        }
    }

    mmio_write32(GICC_EOIR, iar);
    next
}

// ============================================================
// Kernel entry
// ============================================================

fn print_slow(s: &str, delay_ms: u64) {
    for b in s.bytes() {
        if b == b'\n' { Uart.putc(b'\r'); }
        Uart.putc(b);
        if delay_ms > 0 { sleep_ms(delay_ms); }
    }
}

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    clear_screen();

    print_slow("=================================\n", 3);
    print_slow("       AdityaOS v1.0\n", 3);
    print_slow("      Shell + Threads\n", 3);
    print_slow("=================================\n", 3);
    println!();
    println!("Running at EL{}", current_el());
    println!("Timer frequency: {} Hz", read_cntfrq());
    println!();

    let mut buf: [u8; 16] = [0; 16];
    unsafe {
        memset(buf.as_mut_ptr(), b'X' as i32, 8);
        let src = b"WORKS";
        memcpy(buf.as_mut_ptr().add(8), src.as_ptr(), 5);
    }
    print!("memcpy/memset: ");
    for b in buf.iter() {
        if *b == 0 { print!("."); } else { Uart.putc(*b); }
    }
    println!();
    println!();

    println!("Initializing GIC...");
    gic_init();
    println!("Initializing timer...");
    timer_init();
    println!("Setting up scheduler...");
    sched_init();
    println!("Enabling IRQs...");
    enable_irqs();
    println!();

    unsafe {
        start_first(&CTX[0] as *const Context);
    }
}

// ============================================================
// Panic
// ============================================================

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    clear_screen();
    println!("!!! AdityaOS KERNEL PANIC !!!");
    println!();
    println!("{}", info);
    println!();
    let elr: u64;
    unsafe { core::arch::asm!("mrs {}, elr_el1", out(reg) elr, options(nostack, nomem)); }
    println!("Faulting PC: {:#018x}", elr);
    println!("Exception level: EL{}", current_el());
    println!();
    println!("System halted.");
    loop { core::hint::spin_loop(); }
}
