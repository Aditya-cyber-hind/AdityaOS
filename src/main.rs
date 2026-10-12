#![no_std]
#![no_main]

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// ============================================================
// Spinlock (defined, still unused)
// ============================================================

pub struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    pub const fn new() -> Self {
        Self { locked: AtomicBool::new(false) }
    }

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

    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
        unsafe { core::arch::asm!("msr daifclr, #2", options(nostack, nomem)); }
    }
}

#[allow(dead_code)]
static UART_LOCK: SpinLock = SpinLock::new();

// ============================================================
// UART
// ============================================================

const UART0_BASE: usize = 0x0900_0000;
const UART0_DR:   usize = UART0_BASE + 0x00;
const UART0_FR:   usize = UART0_BASE + 0x18;
const UART0_IMSC: usize = UART0_BASE + 0x38;
const UART0_ICR:  usize = UART0_BASE + 0x44;
const UART0_CR:   usize = UART0_BASE + 0x30;

const UART0_TXFF: u32   = 1 << 5;
const UART0_RXFE: u32   = 1 << 4;
const UART0_RXIM: u32   = 1 << 4;

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

// ============================================================
// UART RX ring buffer (lock-free SPSC)
// ============================================================

const RX_BUF_SIZE: usize = 256;

static mut RX_BUF: [u8; RX_BUF_SIZE] = [0; RX_BUF_SIZE];
static RX_HEAD: AtomicUsize = AtomicUsize::new(0);
static RX_TAIL: AtomicUsize = AtomicUsize::new(0);

fn rx_push(byte: u8) {
    let head = RX_HEAD.load(Ordering::Relaxed);
    let tail = RX_TAIL.load(Ordering::Acquire);
    let next = (head + 1) % RX_BUF_SIZE;
    if next == tail { return; }
    unsafe { RX_BUF[head] = byte; }
    RX_HEAD.store(next, Ordering::Release);
}

fn uart_try_getc() -> Option<u8> {
    let tail = RX_TAIL.load(Ordering::Relaxed);
    let head = RX_HEAD.load(Ordering::Acquire);
    if tail == head { return None; }
    let byte = unsafe { RX_BUF[tail] };
    RX_TAIL.store((tail + 1) % RX_BUF_SIZE, Ordering::Release);
    Some(byte)
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
const GICD_ISENABLER1: usize = GICD_BASE + 0x104;
const GICD_IPRIORITYR: usize = GICD_BASE + 0x400;
const GICD_ITARGETSR:  usize = GICD_BASE + 0x800;

const GICC_CTLR: usize = GICC_BASE + 0x000;
const GICC_PMR:  usize = GICC_BASE + 0x004;
const GICC_IAR:  usize = GICC_BASE + 0x00C;
const GICC_EOIR: usize = GICC_BASE + 0x010;

const IRQ_CNTPNS: u32 = 30;
const IRQ_UART0:  u32 = 33;

static TICKS: AtomicU64 = AtomicU64::new(0);
static UART_IRQS: AtomicU64 = AtomicU64::new(0);

fn gic_enable_irq(id: u32) {
    if id < 32 {
        mmio_write32(GICD_ISENABLER0, 1u32 << id);
    } else {
        mmio_write32(GICD_ISENABLER1, 1u32 << (id - 32));
    }
    unsafe {
        let pri = (GICD_IPRIORITYR + id as usize) as *mut u8;
        core::ptr::write_volatile(pri, 0x80);
        let tgt = (GICD_ITARGETSR + id as usize) as *mut u8;
        core::ptr::write_volatile(tgt, 0x01);
    }
}

fn gic_init() {
    mmio_write32(GICD_CTLR, 1);
    mmio_write32(GICC_PMR, 0xFF);
    mmio_write32(GICC_CTLR, 1);
    gic_enable_irq(IRQ_CNTPNS);
    gic_enable_irq(IRQ_UART0);
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

fn uart_enable_rx_irq() {
    mmio_write32(UART0_CR, 0);
    mmio_write32(UART0_ICR, 0x7FF);
    mmio_write32(UART0_IMSC, UART0_RXIM);
    mmio_write32(UART0_CR, (1 << 0) | (1 << 8) | (1 << 9));
}

fn uart_handle_irq() {
    UART_IRQS.fetch_add(1, Ordering::Relaxed);
    loop {
        let fr = mmio_read32(UART0_FR);
        if (fr & UART0_RXFE) != 0 { break; }
        let dr = mmio_read32(UART0_DR);
        rx_push((dr & 0xFF) as u8);
    }
    mmio_write32(UART0_ICR, UART0_RXIM);
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
    Sleeping,
    Dead,
}

#[repr(C)]
pub struct Context {
    pub x: [u64; 31],
    pub sp: u64,
    pub elr: u64,
    pub spsr: u64,
    pub wake_at: u64,
}

const STACK_SIZE: usize = 16 * 1024;
const MAX_THREADS: usize = 8;

const SHELL_SLOT: usize = 2;
const FIRST_SPAWN_SLOT: usize = 3;

#[repr(align(16))]
struct Stack([u8; STACK_SIZE]);

static STACK_A: Stack = Stack([0; STACK_SIZE]);
static STACK_B: Stack = Stack([0; STACK_SIZE]);
static STACK_SHELL: Stack = Stack([0; STACK_SIZE]);

static STACK_S1: Stack = Stack([0; STACK_SIZE]);
static STACK_S2: Stack = Stack([0; STACK_SIZE]);
static STACK_S3: Stack = Stack([0; STACK_SIZE]);
static STACK_S4: Stack = Stack([0; STACK_SIZE]);
static STACK_S5: Stack = Stack([0; STACK_SIZE]);

static mut CTX: [Context; MAX_THREADS] = [
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5, wake_at: 0 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5, wake_at: 0 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5, wake_at: 0 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5, wake_at: 0 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5, wake_at: 0 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5, wake_at: 0 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5, wake_at: 0 },
    Context { x: [0; 31], sp: 0, elr: 0, spsr: 0x5, wake_at: 0 },
];

static mut STATE: [ThreadState; MAX_THREADS] = [ThreadState::Unused; MAX_THREADS];

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);

extern "C" {
    fn start_first(ctx: *const Context) -> !;
}

extern "C" fn thread_a() -> ! { loop { core::hint::spin_loop(); } }
extern "C" fn thread_b() -> ! { loop { core::hint::spin_loop(); } }

/// Cooperative sleep: mark current thread Sleeping until tick+delta.
/// Uses WFI so the CPU actually idles until the next interrupt wakes it.
fn thread_sleep(ticks: u64) {
    unsafe {
        let me = CURRENT.load(Ordering::Relaxed);
        CTX[me].wake_at = TICKS.load(Ordering::Relaxed) + ticks;
        STATE[me] = ThreadState::Sleeping;
    }

    // Wait for the timer IRQ to fire and the scheduler to mark us Ready.
    // WFI waits for the next interrupt without burning CPU.
    loop {
        unsafe { core::arch::asm!("wfi", options(nostack, nomem)); }
        unsafe {
            let me = CURRENT.load(Ordering::Relaxed);
            if STATE[me] != ThreadState::Sleeping {
                break;
            }
        }
    }
}

extern "C" fn spawned_lifecycle_thread() -> ! {
    println!("[spawned: hello]");
    for i in 1..=4 {
        // 5 ticks = ~500ms
        thread_sleep(5);
        println!("[spawned: heartbeat {}]", i);
    }
    println!("[spawned: exiting]");
    thread_exit();
}

pub fn thread_exit() -> ! {
    unsafe {
        let me = CURRENT.load(Ordering::Relaxed);
        STATE[me] = ThreadState::Dead;
    }
    println!("[thread_exit]");
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

        CTX[SHELL_SLOT].sp = STACK_SHELL.0.as_ptr().add(STACK_SIZE) as u64;
        CTX[SHELL_SLOT].elr = shell_thread as *const () as usize as u64;
        CTX[SHELL_SLOT].spsr = 0x5;
        STATE[SHELL_SLOT] = ThreadState::Ready;
    }
}

fn spawn_stack_base(slot: usize) -> Option<usize> {
    match slot {
        3 => Some(STACK_S1.0.as_ptr() as usize),
        4 => Some(STACK_S2.0.as_ptr() as usize),
        5 => Some(STACK_S3.0.as_ptr() as usize),
        6 => Some(STACK_S4.0.as_ptr() as usize),
        7 => Some(STACK_S5.0.as_ptr() as usize),
        _ => None,
    }
}

fn find_free_spawn_slot() -> Option<usize> {
    unsafe {
        for i in FIRST_SPAWN_SLOT..MAX_THREADS {
            if STATE[i] == ThreadState::Dead { return Some(i); }
        }
        for i in FIRST_SPAWN_SLOT..MAX_THREADS {
            if STATE[i] == ThreadState::Unused { return Some(i); }
        }
    }
    None
}

fn spawn_lifecycle_in_slot(slot: usize) -> bool {
    let stack_base = match spawn_stack_base(slot) {
        Some(b) => b,
        None => return false,
    };
    unsafe {
        if STATE[slot] != ThreadState::Unused && STATE[slot] != ThreadState::Dead {
            return false;
        }
        CTX[slot].sp = stack_base as u64 + STACK_SIZE as u64;
        CTX[slot].elr = spawned_lifecycle_thread as *const () as usize as u64;
        CTX[slot].spsr = 0x5;
        CTX[slot].wake_at = 0;
        STATE[slot] = ThreadState::Ready;
    }
    true
}

fn spawn_lifecycle() -> Option<usize> {
    let slot = find_free_spawn_slot()?;
    if spawn_lifecycle_in_slot(slot) { Some(slot) } else { None }
}

fn spawn_many(n: usize) -> usize {
    let mut count = 0;
    for _ in 0..n {
        match spawn_lifecycle() {
            Some(_) => count += 1,
            None => break,
        }
    }
    count
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

fn parse_u64(s: &str) -> Option<u64> {
    s.parse::<u64>().ok()
}

fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(s, 16).ok()
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
            if offset + j < len { Uart.putc(ascii[j]); } else { Uart.putc(b' '); }
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
        ThreadState::Sleeping => "sleep",
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
            println!("  help          - show this message");
            println!("  clear         - clear screen");
            println!("  ticks         - show timer tick count");
            println!("  uptime        - show seconds since boot");
            println!("  time          - same as uptime");
            println!("  threads       - show thread/scheduler info");
            println!("  tasks         - show thread table with state");
            println!("  el            - show current exception level");
            println!("  regs          - dump CPU state");
            println!("  irqs          - show interrupt counts");
            println!("  echo <text>   - print text back");
            println!("  sleep <ms>    - busy-wait sleep");
            println!("  mem <addr>    - dump 128 bytes at hex addr");
            println!("  history       - show command history");
            println!("  spawn         - spawn one lifecycle thread");
            println!("  spawnmany <n> - spawn up to N lifecycle threads");
            println!("  panic         - trigger a kernel panic");
            println!("  reboot        - reset the system (PSCI)");
        }
        "clear" => clear_screen(),
        "ticks" => println!("Ticks: {}", TICKS.load(Ordering::Relaxed)),
        "uptime" | "time" => {
            let secs = read_cntpct() / read_cntfrq();
            println!("Uptime: {} seconds", secs);
        }
        "threads" => {
            let mut live = 0;
            let mut dead = 0;
            let mut sleeping = 0;
            unsafe {
                for i in 0..MAX_THREADS {
                    match STATE[i] {
                        ThreadState::Unused => {}
                        ThreadState::Dead => dead += 1,
                        ThreadState::Sleeping => sleeping += 1,
                        _ => live += 1,
                    }
                }
            }
            println!("Live threads: {}", live);
            println!("Sleeping: {}", sleeping);
            println!("Dead threads: {}", dead);
            println!("Current: {}", CURRENT.load(Ordering::Relaxed));
            println!("Switches: {}", SWITCHES.load(Ordering::Relaxed));
        }
        "tasks" => {
            println!("Idx  State    WakeAt             SP                 ELR");
            println!("---  -------  -----------------  -----------------  -----------------");
            unsafe {
                for i in 0..MAX_THREADS {
                    let s = STATE[i];
                    if s == ThreadState::Unused { continue; }
                    println!("{:>3}  {:<7}  {:<17}  {:#018x}  {:#018x}",
                        i, state_name(s), CTX[i].wake_at, CTX[i].sp, CTX[i].elr);
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
        "irqs" => {
            println!("Timer IRQs: {}", TICKS.load(Ordering::Relaxed));
            println!("UART IRQs:  {}", UART_IRQS.load(Ordering::Relaxed));
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
                Some(idx) => println!("Spawned lifecycle thread in slot {}", idx),
                None => println!("No free spawn slots."),
            }
        }
        "spawnmany" => {
            if let Some(n) = parse_u64(args) {
                let n = n.min(10) as usize;
                let count = spawn_many(n);
                println!("Spawned {} threads.", count);
            } else {
                println!("Usage: spawnmany <n>");
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

    // Wait a few ticks before printing the banner
    thread_sleep(5);

    println!();
    println!("=================================");
    println!("     AdityaOS Shell v1.5");
    println!("     Sleeping Threads");
    println!("=================================");
    println!();

    shell_prompt();

    loop {
        let c = match uart_try_getc() {
            Some(b) => b,
            None => {
                // Sleep 1 tick and try again. WFI means the CPU idles.
                thread_sleep(1);
                continue;
            }
        };

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

        // Wake any Sleeping threads whose wake_at <= current tick
        unsafe {
            for i in 0..MAX_THREADS {
                if STATE[i] == ThreadState::Sleeping && CTX[i].wake_at <= t {
                    STATE[i] = ThreadState::Ready;
                }
            }
        }

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
    } else if id == IRQ_UART0 {
        uart_handle_irq();
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
    print_slow("       AdityaOS v1.5\n", 3);
    print_slow("     Sleeping Threads\n", 3);
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
    println!("Enabling UART RX interrupts...");
    uart_enable_rx_irq();
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
