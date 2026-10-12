#![no_std]
#![no_main]

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// ============================================================
// UART — raw (unlocked) primitives
// ============================================================

const UART0_BASE: usize = 0x0900_0000;
const UART0_DR:   usize = UART0_BASE + 0x00;
const UART0_FR:   usize = UART0_BASE + 0x18;
const UART0_TXFF: u32   = 1 << 5;
const UART0_RXFE: u32   = 1 << 4;

/// Write a byte to UART with no locking.
/// Safe to call from panic/exception contexts, but not concurrent-safe.
fn uart_putc_raw(c: u8) {
    unsafe {
        while (core::ptr::read_volatile(UART0_FR as *const u32) & UART0_TXFF) != 0 {}
        core::ptr::write_volatile(UART0_DR as *mut u32, c as u32);
    }
}

/// Write a string to UART with no locking.
/// Used by panic and exception handlers, which cannot rely on locks.
fn panic_puts(s: &str) {
    for b in s.bytes() {
        if b == b'\n' { uart_putc_raw(b'\r'); }
        uart_putc_raw(b);
    }
}

/// Write a u64 as hex, prefixed with 0x, into UART with no locking.
fn panic_puthex(v: u64) {
    panic_puts("0x");
    let mut started = false;
    let mut shift: i32 = 60;
    while shift >= 0 {
        let nib = ((v >> shift) & 0xF) as u8;
        if nib != 0 || started || shift == 0 {
            started = true;
            let c = if nib < 10 { b'0' + nib } else { b'a' + (nib - 10) };
            uart_putc_raw(c);
        }
        shift -= 4;
    }
}

// ============================================================
// Spinlock
// ============================================================

pub struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    pub const fn new() -> Self {
        Self { locked: AtomicBool::new(false) }
    }

    /// Acquire, disabling IRQs while held.
    pub fn lock(&self) {
        unsafe { core::arch::asm!("msr daifset, #2", options(nostack, nomem)); }

        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    /// Release, restoring IRQs.
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
        unsafe { core::arch::asm!("msr daifclr, #2", options(nostack, nomem)); }
    }
}

static UART_LOCK: SpinLock = SpinLock::new();

// ============================================================
// UART — locked writer, used by println!
// ============================================================

pub struct Uart;

impl Uart {
    fn putc_unlocked(&self, c: u8) {
        uart_putc_raw(c);
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Lock for the entire string so a whole line is atomic.
        UART_LOCK.lock();
        for b in s.bytes() {
            if b == b'\n' { self.putc_unlocked(b'\r'); }
            self.putc_unlocked(b);
        }
        UART_LOCK.unlock();
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

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ThreadState {
    Unused,
    Ready,
    Running,
    Dead,
}

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

static mut STATE: [ThreadState; MAX_THREADS] = [ThreadState::Unused; MAX_THREADS];

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);

extern "C" {
    fn start_first(ctx: *const Context) -> !;
}

extern "C" fn thread_a() -> ! { loop { core::hint::spin_loop(); } }
extern "C" fn thread_b() -> ! { loop { core::hint::spin_loop(); } }

static SPAM_LINES: AtomicUsize = AtomicUsize::new(0);

extern "C" fn spam_thread() -> ! {
    let n = SPAM_LINES.load(Ordering::Relaxed);
    for i in 1..=n {
        println!("[spam {:>2}] This is a long line printed by a spawned thread, number {}.", i, i);
    }
    println!("[spam done]");
    thread_exit();
}

extern "C" fn spawned_lifecycle_thread() -> ! {
    println!("[thread started: will run for ~2s]");
    for i in 1..=4 {
        sleep_ms(500);
        println!("[thread heartbeat {}]", i);
    }
    println!("[thread exiting]");
    thread_exit();
}

pub fn thread_exit() -> ! {
    unsafe {
        let me = CURRENT.load(Ordering::Relaxed);
        STATE[me] = ThreadState::Dead;
    }
    println!("[thread_exit: marked dead, yielding]");
    loop { core::hint::spin_loop(); }
}

pub fn sched_init() {
    unsafe {
        CTX[0].sp = STACK_A.0.as_ptr().add(STACK_SIZE) as u64;
        CTX[0].elr = thread_a as *const () as usize as u64;
        CTX[0].spsr = 0x5;
        STATE[0] = ThreadState::Ready;

        CTX[1].sp = STACK_B.0.as_ptr().add(STACK_SIZE) as u64;
        CTX[1].elr = thread_b as *const () as usize as u64;
        CTX[1].spsr = 0x5;
        STATE[1] = ThreadState::Ready;

        CTX[2].sp = STACK_SHELL.0.as_ptr().add(STACK_SIZE) as u64;
        CTX[2].elr = shell_thread as *const () as usize as u64;
        CTX[2].spsr = 0x5;
        STATE[2] = ThreadState::Ready;
    }
}

fn find_free_slot() -> Option<usize> {
    unsafe {
        for i in 0..MAX_THREADS {
            if STATE[i] == ThreadState::Dead { return Some(i); }
        }
        for i in 0..MAX_THREADS {
            if STATE[i] == ThreadState::Unused { return Some(i); }
        }
    }
    None
}

fn spawn_thread_with_entry(entry: usize) -> Option<usize> {
    let idx = find_free_slot()?;
    if idx < 3 { return None; }
    let stack_idx = idx - 3;
    let stack_base = spawn_stack_base(stack_idx)?;

    unsafe {
        CTX[idx].sp = stack_base as u64 + STACK_SIZE as u64;
        CTX[idx].elr = entry as u64;
        CTX[idx].spsr = 0x5;
        STATE[idx] = ThreadState::Ready;
    }
    Some(idx)
}

fn spawn_lifecycle() -> Option<usize> {
    spawn_thread_with_entry(spawned_lifecycle_thread as *const () as usize)
}

fn spawn_spam(n: usize) -> Option<usize> {
    SPAM_LINES.store(n, Ordering::Relaxed);
    spawn_thread_with_entry(spam_thread as *const () as usize)
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
        while i < 16 { print!("   "); i += 1; }
        print!(" |");
        for j in 0..16 {
            if offset + j < len { uart_putc_raw(ascii[j]); } else { uart_putc_raw(b' '); }
        }
        println!("|");
        offset += 16;
    }
}

fn state_name(s: ThreadState) -> &'static str {
    match s {
        ThreadState::Unused => "unused",
        ThreadState::Ready => "ready",
        ThreadState::Running => "running",
        ThreadState::Dead => "dead",
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
            println!("  time         - same as uptime");
            println!("  threads      - show thread/scheduler info");
            println!("  tasks        - show per-thread state table");
            println!("  el           - show current exception level");
            println!("  regs         - dump CPU state");
            println!("  echo <text>  - print text back");
            println!("  sleep <ms>   - sleep for N milliseconds");
            println!("  mem <addr>   - dump 128 bytes at hex addr");
            println!("  history      - show command history");
            println!("  spawn        - spawn a lifecycle thread (~2s)");
            println!("  spam <n>     - spawn a thread printing N lines (lock test)");
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
            let mut live = 0;
            unsafe {
                for i in 0..MAX_THREADS {
                    if STATE[i] != ThreadState::Unused { live += 1; }
                }
            }
            println!("Threads alive: {}", live);
            println!("Current: {}", CURRENT.load(Ordering::Relaxed));
            println!("Switches: {}", SWITCHES.load(Ordering::Relaxed));
        }
        "tasks" => {
            println!("Idx  State    SP                 ELR");
            println!("---  -------  -----------------  -----------------");
            unsafe {
                for i in 0..MAX_THREADS {
                    let s = STATE[i];
                    if s == ThreadState::Unused { continue; }
                    println!("{:>3}  {:<7}  {:#018x}  {:#018x}",
                        i, state_name(s), CTX[i].sp, CTX[i].elr);
                }
            }
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
            match spawn_lifecycle() {
                Some(idx) => println!("Spawned lifecycle thread index {}", idx),
                None => println!("No free thread slots."),
            }
        }
        "spam" => {
            if let Some(n) = parse_u64(args) {
                if n == 0 || n > 50 {
                    println!("Use spam 1..50");
                } else {
                    match spawn_spam(n as usize) {
                        Some(idx) => println!("Spawned spam thread index {} ({} lines)", idx, n),
                        None => println!("No free thread slots."),
                    }
                }
            } else {
                println!("Usage: spam <n>   (n = 1..50)");
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
    println!("     AdityaOS Shell v1.3");
    println!("   Spinlock + Safe Panic");
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
                    print!("{}", c as char);
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
    // Lock-free output only.
    panic_puts("\n=== AdityaOS Exception ===\n");
    panic_puts("  ESR_EL1: "); panic_puthex(esr); panic_puts("\n");
    panic_puts("  ELR_EL1: "); panic_puthex(elr); panic_puts("\n");
    panic_puts("  FAR_EL1: "); panic_puthex(far); panic_puts("\n");
    panic_puts("  EC:      "); panic_puthex((esr >> 26) & 0x3F); panic_puts("\n");
    panic_puts("System halted.\n");
    loop { core::hint::spin_loop(); }
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
            unsafe {
                if STATE[cur] == ThreadState::Running {
                    STATE[cur] = ThreadState::Ready;
                }
            }
            let mut nxt = cur;
            unsafe {
                for step in 1..=MAX_THREADS {
                    let cand = (cur + step) % MAX_THREADS;
                    if STATE[cand] == ThreadState::Ready || STATE[cand] == ThreadState::Running {
                        nxt = cand;
                        break;
                    }
                }
                STATE[nxt] = ThreadState::Running;
                next = &CTX[nxt] as *const Context;
            }
            CURRENT.store(nxt, Ordering::Relaxed);
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
        if b == b'\n' { uart_putc_raw(b'\r'); }
        uart_putc_raw(b);
        if delay_ms > 0 { sleep_ms(delay_ms); }
    }
}

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    clear_screen();

    print_slow("=================================\n", 3);
    print_slow("       AdityaOS v1.3\n", 3);
    print_slow("   Spinlock + Safe Panic\n", 3);
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
        if *b == 0 { print!("."); } else { uart_putc_raw(*b); }
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
        STATE[0] = ThreadState::Running;
        start_first(&CTX[0] as *const Context);
    }
}

// ============================================================
// Panic
// ============================================================

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Lock-free output only. Do NOT use println! here.
    panic_puts("\n!!! AdityaOS KERNEL PANIC !!!\n\n");

    // Print panic message manually without allocating.
    if let Some(loc) = info.location() {
        panic_puts("at ");
        panic_puts(loc.file());
        panic_puts(":");
        // Approximate line number as decimal
        let mut line = loc.line();
        let mut digits = [0u8; 10];
        let mut n = 0;
        if line == 0 { digits[0] = b'0'; n = 1; }
        while line > 0 {
            digits[n] = b'0' + (line % 10) as u8;
            line /= 10;
            n += 1;
        }
        while n > 0 { n -= 1; uart_putc_raw(digits[n]); }
        panic_puts("\n");
    }

    // If the panic message is a known simple str, print it directly.
    // Otherwise, skip it — we cannot safely format here.
    let msg = info.message();
    let s = msg.as_str().unwrap_or("<non-str panic message>");
    panic_puts("message: ");
    panic_puts(s);
    panic_puts("\n\n");

    // Read fault diagnostics
    let elr: u64;
    unsafe { core::arch::asm!("mrs {}, elr_el1", out(reg) elr, options(nostack, nomem)); }
    let cel: u64;
    unsafe { core::arch::asm!("mrs {}, currentel", out(reg) cel, options(nostack, nomem)); }

    panic_puts("Faulting PC: "); panic_puthex(elr); panic_puts("\n");
    panic_puts("Exception level: EL");
    let el = ((cel >> 2) & 0x3) as u8;
    uart_putc_raw(b'0' + el);
    panic_puts("\n\n");

    panic_puts("System halted.\n");
    loop { core::hint::spin_loop(); }
}
