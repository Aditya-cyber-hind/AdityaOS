#![no_std]
#![no_main]

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

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
// ANSI color codes
// ============================================================

const RESET:   &str = "\x1b[0m";
const BOLD:    &str = "\x1b[1m";
const RED:     &str = "\x1b[31m";
const GREEN:   &str = "\x1b[32m";
const YELLOW:  &str = "\x1b[33m";
const BLUE:    &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const CYAN:    &str = "\x1b[36m";
const WHITE:   &str = "\x1b[37m";

// ============================================================
// UART
// ============================================================

const UART0_BASE: usize = 0x0900_0000;
const UART0_DR:   usize = UART0_BASE + 0x00;
const UART0_FR:   usize = UART0_BASE + 0x18;
const UART0_IMSC: usize = UART0_BASE + 0x38;
const UART0_ICR:  usize = UART0_BASE + 0x44;
const UART0_CR:   usize = UART0_BASE + 0x30;

const UART0_TXFF: u32 = 1 << 5;
const UART0_RXFE: u32 = 1 << 4;
const UART0_RXIM: u32 = 1 << 4;

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
// UART RX ring buffer
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
// Heap allocator (bump)
// ============================================================

const HEAP_START: usize = 0x4800_0000;
const HEAP_END:   usize = 0x4F00_0000;
const HEAP_SIZE:  usize = HEAP_END - HEAP_START;

static HEAP_NEXT: AtomicUsize = AtomicUsize::new(0);
static HEAP_ALLOCS: AtomicUsize = AtomicUsize::new(0);

fn heap_init() {
    HEAP_NEXT.store(HEAP_START, Ordering::Relaxed);
    HEAP_ALLOCS.store(0, Ordering::Relaxed);
}

fn heap_used() -> usize {
    let next = HEAP_NEXT.load(Ordering::Relaxed);
    if next == 0 { 0 } else { next - HEAP_START }
}

pub fn kmalloc(size: usize, align: usize) -> *mut u8 {
    if size == 0 || align == 0 { return core::ptr::null_mut(); }

    loop {
        let cur = HEAP_NEXT.load(Ordering::Relaxed);
        if cur == 0 { return core::ptr::null_mut(); }

        let aligned = (cur + align - 1) & !(align - 1);
        let end = aligned + size;

        if end > HEAP_END { return core::ptr::null_mut(); }

        match HEAP_NEXT.compare_exchange_weak(
            cur, end, Ordering::AcqRel, Ordering::Relaxed)
        {
            Ok(_) => {
                HEAP_ALLOCS.fetch_add(1, Ordering::Relaxed);
                return aligned as *mut u8;
            }
            Err(_) => continue,
        }
    }
}

pub fn kfree(_ptr: *mut u8) {}

// ============================================================
// Page tables + MMU
// ============================================================

const DESC_VALID:  u64 = 1 << 0;
const DESC_TABLE:  u64 = 1 << 1;
const DESC_AF:     u64 = 1 << 10;
const DESC_SH_INNER: u64 = 0b11 << 8;

const DESC_ATTR_NORMAL: u64 = 0 << 2;
const DESC_ATTR_DEVICE: u64 = 1 << 2;

const BLOCK_NORMAL: u64 = DESC_VALID | DESC_AF | DESC_SH_INNER | DESC_ATTR_NORMAL;
const BLOCK_DEVICE: u64 = DESC_VALID | DESC_AF | DESC_ATTR_DEVICE;

const TABLE_ENTRY: u64 = DESC_VALID | DESC_TABLE;
const NEXT_TABLE: u64 = DESC_VALID | DESC_TABLE | DESC_AF;
const L3_PAGE_NORMAL: u64 = DESC_VALID | DESC_TABLE | DESC_AF | DESC_SH_INNER | DESC_ATTR_NORMAL;

#[repr(C, align(4096))]
struct PageTable([u64; 512]);

const MAX_THREADS: usize = 8;

static mut TT_KERNEL_L0: PageTable = PageTable([0; 512]);
static mut TT_KERNEL_L1: PageTable = PageTable([0; 512]);

static mut TT_THREAD_L0: [PageTable; MAX_THREADS] =
    [PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]),
     PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512])];

static mut TT_THREAD_L1: [PageTable; MAX_THREADS] =
    [PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]),
     PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512])];

static mut TT_THREAD_L2: [PageTable; MAX_THREADS] =
    [PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]),
     PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512])];

static mut TT_THREAD_L3: [PageTable; MAX_THREADS] =
    [PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]),
     PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512]), PageTable([0; 512])];

static TT_KERNEL_L0_PHYS: AtomicUsize = AtomicUsize::new(0);
static TT_THREAD_L0_PHYS: [AtomicUsize; MAX_THREADS] = [
    AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0),
];

fn kernel_page_tables_init() {
    unsafe {
        let l0 = core::ptr::addr_of_mut!(TT_KERNEL_L0) as *mut u64;
        let l1 = core::ptr::addr_of_mut!(TT_KERNEL_L1) as *mut u64;
        *l0 = (l1 as u64) | TABLE_ENTRY;

        let l1_slice = core::ptr::addr_of_mut!(TT_KERNEL_L1.0) as *mut u64;
        *l1_slice.add(0) = 0x0000_0000u64 | BLOCK_DEVICE;
        *l1_slice.add(1) = 0x4000_0000u64 | BLOCK_NORMAL;
        for i in 2..512 {
            *l1_slice.add(i) = 0;
        }

        TT_KERNEL_L0_PHYS.store(l0 as usize, Ordering::Relaxed);
    }
}

fn tt_kernel_l0_phys() -> usize {
    TT_KERNEL_L0_PHYS.load(Ordering::Relaxed)
}

fn thread_page_tables_init() {
    unsafe {
        let kernel_l1 = core::ptr::addr_of!(TT_KERNEL_L1) as usize;

        for i in 0..MAX_THREADS {
            let l0 = core::ptr::addr_of_mut!(TT_THREAD_L0[i]) as *mut u64;
            *l0 = (kernel_l1 as u64) | TABLE_ENTRY;
            let l1 = core::ptr::addr_of_mut!(TT_THREAD_L1[i]) as *mut u64;
            *l0.add(1) = (l1 as u64) | TABLE_ENTRY;
            for j in 2..512 {
                *l0.add(j) = 0;
            }

            for j in 0..512 {
                *l1.add(j) = 0;
            }

            let l2 = core::ptr::addr_of_mut!(TT_THREAD_L2[i]) as *mut u64;
            for j in 0..512 {
                *l2.add(j) = 0;
            }

            let l3 = core::ptr::addr_of_mut!(TT_THREAD_L3[i]) as *mut u64;
            for j in 0..512 {
                *l3.add(j) = 0;
            }

            TT_THREAD_L0_PHYS[i].store(l0 as usize, Ordering::Relaxed);
        }
    }
}

fn tt_thread_l0_phys(idx: usize) -> usize {
    if idx < MAX_THREADS {
        TT_THREAD_L0_PHYS[idx].load(Ordering::Relaxed)
    } else {
        0
    }
}

fn thread_map_page(thread_idx: usize, vaddr: u64, paddr: u64) -> bool {
    if thread_idx >= MAX_THREADS { return false; }

    let l0_idx = ((vaddr >> 39) & 0x1FF) as usize;
    let l1_idx = ((vaddr >> 30) & 0x1FF) as usize;
    let l2_idx = ((vaddr >> 21) & 0x1FF) as usize;
    let l3_idx = ((vaddr >> 12) & 0x1FF) as usize;

    if l0_idx != 1 { return false; }

    unsafe {
        let l0 = core::ptr::addr_of_mut!(TT_THREAD_L0[thread_idx]) as *mut u64;
        let l1 = core::ptr::addr_of_mut!(TT_THREAD_L1[thread_idx]) as *mut u64;
        let l2 = core::ptr::addr_of_mut!(TT_THREAD_L2[thread_idx]) as *mut u64;
        let l3 = core::ptr::addr_of_mut!(TT_THREAD_L3[thread_idx]) as *mut u64;

        *l0.add(1) = (l1 as u64) | TABLE_ENTRY;

        if l1_idx != 0 { return false; }
        *l1.add(l1_idx) = (l2 as u64) | NEXT_TABLE;

        *l2.add(l2_idx) = (l3 as u64) | NEXT_TABLE;

        *l3.add(l3_idx) = (paddr & !0xFFFu64) | L3_PAGE_NORMAL;

        core::arch::asm!("dsb ish", options(nostack, nomem));
        core::arch::asm!("tlbi vmalle1", options(nostack, nomem));
        core::arch::asm!("dsb ish", options(nostack, nomem));
        core::arch::asm!("isb", options(nostack, nomem));
    }
    true
}

fn set_ttbr0(phys: u64) {
    unsafe {
        core::arch::asm!("msr ttbr0_el1, {v}", v = in(reg) phys, options(nostack, nomem));
        core::arch::asm!("dsb ish", options(nostack, nomem));
        core::arch::asm!("tlbi vmalle1", options(nostack, nomem));
        core::arch::asm!("dsb ish", options(nostack, nomem));
        core::arch::asm!("isb", options(nostack, nomem));
    }
}

const MAIR_VALUE: u64 = 0x0000_0000_0000_04FF;
const TCR_VALUE: u64 = 0x0000_0000_3080_3510;

fn mmu_configure() {
    let ttbr0 = tt_kernel_l0_phys() as u64;
    unsafe {
        core::arch::asm!("msr mair_el1, {v}", v = in(reg) MAIR_VALUE, options(nostack, nomem));
        core::arch::asm!("msr tcr_el1,  {v}", v = in(reg) TCR_VALUE,  options(nostack, nomem));
        core::arch::asm!("msr ttbr0_el1, {v}", v = in(reg) ttbr0,     options(nostack, nomem));
        core::arch::asm!("isb", options(nostack, nomem));
    }
}

fn mmu_enable() {
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, nomem));
        core::arch::asm!("isb", options(nostack, nomem));
        core::arch::asm!("tlbi vmalle1", options(nostack, nomem));
        core::arch::asm!("dsb ish", options(nostack, nomem));
        core::arch::asm!("isb", options(nostack, nomem));

        let mut sctlr: u64;
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nostack, nomem));
        sctlr |= 1;
        core::arch::asm!("msr sctlr_el1, {}", in(reg) sctlr, options(nostack, nomem));
        core::arch::asm!("isb", options(nostack, nomem));
    }
}

fn read_mair() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, mair_el1", out(reg) v, options(nostack, nomem)); }
    v
}

fn read_tcr() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, tcr_el1", out(reg) v, options(nostack, nomem)); }
    v
}

fn read_ttbr0() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, ttbr0_el1", out(reg) v, options(nostack, nomem)); }
    v
}

fn read_sctlr() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, sctlr_el1", out(reg) v, options(nostack, nomem)); }
    v
}

// ============================================================
// RAM filesystem
// ============================================================

const FS_BLOCK_SIZE: usize = 512;
const FS_MAX_BLOCKS: usize = 64;
const FS_MAX_FILES: usize = 32;
const FS_MAX_NAME: usize = 32;
const FS_MAX_BLOCKS_PER_FILE: usize = 4;
const FS_MAGIC: u32 = 0x4164_4F53;

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum FsNodeType {
    Empty,
    File,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FsInode {
    pub node_type: FsNodeType,
    pub name: [u8; FS_MAX_NAME],
    pub name_len: usize,
    pub size: usize,
    pub blocks: [u16; FS_MAX_BLOCKS_PER_FILE],
    pub block_count: usize,
}

#[repr(C)]
pub struct FsState {
    pub magic: u32,
    pub formatted: bool,
    pub total_blocks: usize,
    pub used_blocks: usize,
    pub inodes: [FsInode; FS_MAX_FILES],
    pub blocks: [[u8; FS_BLOCK_SIZE]; FS_MAX_BLOCKS],
}

static mut FS: FsState = FsState {
    magic: 0,
    formatted: false,
    total_blocks: FS_MAX_BLOCKS,
    used_blocks: 0,
    inodes: [FsInode {
        node_type: FsNodeType::Empty,
        name: [0; FS_MAX_NAME],
        name_len: 0,
        size: 0,
        blocks: [0; FS_MAX_BLOCKS_PER_FILE],
        block_count: 0,
    }; FS_MAX_FILES],
    blocks: [[0; FS_BLOCK_SIZE]; FS_MAX_BLOCKS],
};

fn fs_format() -> bool {
    unsafe {
        FS.magic = FS_MAGIC;
        FS.formatted = true;
        FS.total_blocks = FS_MAX_BLOCKS;
        FS.used_blocks = 0;
        for i in 0..FS_MAX_FILES {
            FS.inodes[i].node_type = FsNodeType::Empty;
            FS.inodes[i].name = [0; FS_MAX_NAME];
            FS.inodes[i].name_len = 0;
            FS.inodes[i].size = 0;
            FS.inodes[i].blocks = [0; FS_MAX_BLOCKS_PER_FILE];
            FS.inodes[i].block_count = 0;
        }
        for i in 0..FS_MAX_BLOCKS {
            for j in 0..FS_BLOCK_SIZE {
                FS.blocks[i][j] = 0;
            }
        }
    }
    true
}

fn fs_is_formatted() -> bool {
    unsafe { FS.formatted }
}

fn fs_name_to_bytes(name: &str) -> ([u8; FS_MAX_NAME], usize) {
    let mut buf = [0u8; FS_MAX_NAME];
    let bytes = name.as_bytes();
    let n = bytes.len().min(FS_MAX_NAME - 1);
    let mut i = 0;
    while i < n {
        buf[i] = bytes[i];
        i += 1;
    }
    (buf, n)
}

fn fs_names_equal(inode: &FsInode, name: &str) -> bool {
    let bytes = name.as_bytes();
    if inode.name_len != bytes.len() { return false; }
    let mut i = 0;
    while i < inode.name_len {
        if inode.name[i] != bytes[i] { return false; }
        i += 1;
    }
    true
}

fn fs_find(name: &str) -> Option<usize> {
    unsafe {
        for i in 0..FS_MAX_FILES {
            if FS.inodes[i].node_type == FsNodeType::Empty { continue; }
            if fs_names_equal(&FS.inodes[i], name) {
                return Some(i);
            }
        }
    }
    None
}

fn fs_find_free_inode() -> Option<usize> {
    unsafe {
        for i in 0..FS_MAX_FILES {
            if FS.inodes[i].node_type == FsNodeType::Empty {
                return Some(i);
            }
        }
    }
    None
}

fn fs_find_free_block() -> Option<usize> {
    unsafe {
        for i in 0..FS_MAX_BLOCKS {
            let mut used = false;
            for j in 0..FS_MAX_FILES {
                if FS.inodes[j].node_type == FsNodeType::Empty { continue; }
                for k in 0..FS.inodes[j].block_count {
                    if FS.inodes[j].blocks[k] as usize == i {
                        used = true;
                        break;
                    }
                }
                if used { break; }
            }
            if !used { return Some(i); }
        }
    }
    None
}

fn fs_create(name: &str) -> Result<usize, &'static str> {
    if !fs_is_formatted() {
        return Err("filesystem not formatted (run mkfs)");
    }
    if fs_find(name).is_some() {
        return Err("file already exists");
    }
    if name.is_empty() || name.len() >= FS_MAX_NAME {
        return Err("invalid name");
    }
    let idx = match fs_find_free_inode() {
        Some(i) => i,
        None => return Err("no free inodes"),
    };
    let (bytes, n) = fs_name_to_bytes(name);
    unsafe {
        FS.inodes[idx].node_type = FsNodeType::File;
        FS.inodes[idx].name = bytes;
        FS.inodes[idx].name_len = n;
        FS.inodes[idx].size = 0;
        FS.inodes[idx].blocks = [0; FS_MAX_BLOCKS_PER_FILE];
        FS.inodes[idx].block_count = 0;
    }
    Ok(idx)
}

fn fs_write(idx: usize, data: &[u8]) -> Result<usize, &'static str> {
    unsafe {
        if idx >= FS_MAX_FILES || FS.inodes[idx].node_type == FsNodeType::Empty {
            return Err("invalid file");
        }

        let needed_blocks = (data.len() + FS_BLOCK_SIZE - 1) / FS_BLOCK_SIZE;
        if needed_blocks > FS_MAX_BLOCKS_PER_FILE {
            return Err("file too large (max 4 blocks = 2KB)");
        }

        FS.inodes[idx].block_count = 0;

        let mut remaining = data.len();
        let mut offset = 0;
        let mut block_num = 0;

        while remaining > 0 && block_num < needed_blocks {
            let blk = match fs_find_free_block() {
                Some(b) => b,
                None => return Err("out of disk space"),
            };
            FS.inodes[idx].blocks[block_num] = blk as u16;
            block_num += 1;

            let to_copy = if remaining > FS_BLOCK_SIZE { FS_BLOCK_SIZE } else { remaining };
            let mut j = 0;
            while j < to_copy {
                FS.blocks[blk][j] = data[offset + j];
                j += 1;
            }
            offset += to_copy;
            remaining -= to_copy;
        }

        FS.inodes[idx].block_count = block_num;
        FS.inodes[idx].size = data.len();
        Ok(data.len())
    }
}

fn fs_append(idx: usize, data: &[u8]) -> Result<usize, &'static str> {
    unsafe {
        if idx >= FS_MAX_FILES || FS.inodes[idx].node_type == FsNodeType::Empty {
            return Err("invalid file");
        }

        let old_size = FS.inodes[idx].size;
        let new_size = old_size + data.len();
        let needed_blocks = (new_size + FS_BLOCK_SIZE - 1) / FS_BLOCK_SIZE;

        if needed_blocks > FS_MAX_BLOCKS_PER_FILE {
            return Err("file too large (max 4 blocks = 2KB)");
        }

        // Determine current block count (may need to grow)
        let old_block_count = FS.inodes[idx].block_count;

        // Allocate new blocks if needed
        let mut block_num = old_block_count;
        while block_num < needed_blocks {
            let blk = match fs_find_free_block() {
                Some(b) => b,
                None => return Err("out of disk space"),
            };
            FS.inodes[idx].blocks[block_num] = blk as u16;
            block_num += 1;
        }

        // Write bytes at old_size position
        let mut remaining = data.len();
        let mut src_off = 0;
        let mut pos = old_size;

        while remaining > 0 {
            let blk_idx = pos / FS_BLOCK_SIZE;
            let off_in_blk = pos % FS_BLOCK_SIZE;
            let blk = FS.inodes[idx].blocks[blk_idx] as usize;
            let to_copy = {
                let space_left = FS_BLOCK_SIZE - off_in_blk;
                if remaining > space_left { space_left } else { remaining }
            };
            let mut j = 0;
            while j < to_copy {
                FS.blocks[blk][off_in_blk + j] = data[src_off + j];
                j += 1;
            }
            pos += to_copy;
            src_off += to_copy;
            remaining -= to_copy;
        }

        FS.inodes[idx].block_count = block_num;
        FS.inodes[idx].size = new_size;
        Ok(data.len())
    }
}

fn fs_read(idx: usize, out: &mut [u8]) -> Result<usize, &'static str> {
    unsafe {
        if idx >= FS_MAX_FILES || FS.inodes[idx].node_type == FsNodeType::Empty {
            return Err("invalid file");
        }
        let size = FS.inodes[idx].size;
        let copy_len = if out.len() < size { out.len() } else { size };

        let mut remaining = copy_len;
        let mut out_off = 0;
        for b in 0..FS.inodes[idx].block_count {
            if remaining == 0 { break; }
            let blk = FS.inodes[idx].blocks[b] as usize;
            let to_copy = if remaining > FS_BLOCK_SIZE { FS_BLOCK_SIZE } else { remaining };
            let mut j = 0;
            while j < to_copy {
                out[out_off + j] = FS.blocks[blk][j];
                j += 1;
            }
            out_off += to_copy;
            remaining -= to_copy;
        }
        Ok(copy_len)
    }
}

fn fs_delete(name: &str) -> Result<(), &'static str> {
    let idx = match fs_find(name) {
        Some(i) => i,
        None => return Err("file not found"),
    };
    unsafe {
        FS.inodes[idx].node_type = FsNodeType::Empty;
        FS.inodes[idx].name = [0; FS_MAX_NAME];
        FS.inodes[idx].name_len = 0;
        FS.inodes[idx].size = 0;
        FS.inodes[idx].block_count = 0;
    }
    Ok(())
}

fn fs_count_files() -> usize {
    let mut count = 0;
    unsafe {
        for i in 0..FS_MAX_FILES {
            if FS.inodes[i].node_type != FsNodeType::Empty { count += 1; }
        }
    }
    count
}

fn fs_count_used_blocks() -> usize {
    let mut count = 0;
    unsafe {
        for i in 0..FS_MAX_FILES {
            if FS.inodes[i].node_type == FsNodeType::Empty { continue; }
            count += FS.inodes[i].block_count;
        }
    }
    count
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

fn thread_sleep(ticks: u64) {
    unsafe {
        let me = CURRENT.load(Ordering::Relaxed);
        CTX[me].wake_at = TICKS.load(Ordering::Relaxed) + ticks;
        STATE[me] = ThreadState::Sleeping;
    }
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
// VM isolation demo
// ============================================================

const DEMO_VA: u64 = 0x80_0000_0000 + 0x20_0000;

static mut DEMO_PHYS: [usize; MAX_THREADS] = [0; MAX_THREADS];

extern "C" fn vm_demo_thread() -> ! {
    let me = CURRENT.load(Ordering::Relaxed);

    let page = kmalloc(4096, 4096);
    if page.is_null() {
        println!("[vm-demo {}] kmalloc failed", me);
        thread_exit();
    }
    let paddr = page as u64;

    if !thread_map_page(me, DEMO_VA, paddr) {
        println!("[vm-demo {}] map failed", me);
        thread_exit();
    }

    unsafe {
        DEMO_PHYS[me] = page as usize;
    }

    let pattern: u64 = 0xAA00_0000_0000_0000u64 | ((me as u64) << 40);

    unsafe {
        let p = DEMO_VA as *mut u64;
        core::ptr::write_volatile(p, pattern);
    }

    println!("[vm-demo {}] wrote {:#018x} to VA {:#x} (phys {:#x})",
        me, pattern, DEMO_VA, paddr);

    thread_sleep(8);

    let readback = unsafe {
        let p = DEMO_VA as *const u64;
        core::ptr::read_volatile(p)
    };

    if readback == pattern {
        println!("{}[vm-demo {}] read back {:#018x}  OK (isolation works){}",
            GREEN, me, readback, RESET);
    } else {
        println!("{}[vm-demo {}] read back {:#018x}  WRONG (expected {:#018x}){}",
            RED, me, readback, pattern, RESET);
    }

    thread_exit();
}

fn spawn_vm_demo() -> Option<usize> {
    let slot = find_free_spawn_slot()?;
    let stack_base = spawn_stack_base(slot)?;

    unsafe {
        let l1 = core::ptr::addr_of_mut!(TT_THREAD_L1[slot]) as *mut u64;
        let l2 = core::ptr::addr_of_mut!(TT_THREAD_L2[slot]) as *mut u64;
        let l3 = core::ptr::addr_of_mut!(TT_THREAD_L3[slot]) as *mut u64;
        for i in 0..512 {
            *l1.add(i) = 0;
            *l2.add(i) = 0;
            *l3.add(i) = 0;
        }
    }

    unsafe {
        CTX[slot].sp = stack_base as u64 + STACK_SIZE as u64;
        CTX[slot].elr = vm_demo_thread as *const () as usize as u64;
        CTX[slot].spsr = 0x5;
        CTX[slot].wake_at = 0;
        STATE[slot] = ThreadState::Ready;
    }
    Some(slot)
}

// ============================================================
// Shell
// ============================================================

const MAX_LINE: usize = 128;
const HISTORY_SIZE: usize = 16;

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

fn shell_prompt() { print!("{}adityaos>{} ", CYAN, RESET); }

fn redraw_line(buf: &[u8], len: usize, cursor: usize) {
    print!("\r\x1b[2K");
    print!("{}adityaos>{} ", CYAN, RESET);
    let mut i = 0;
    while i < len {
        Uart.putc(buf[i]);
        i += 1;
    }
    if cursor < len {
        let back = len - cursor;
        let mut j = 0;
        while j < back {
            print!("\x1b[D");
            j += 1;
        }
    }
}

unsafe fn history_get(offset: usize) -> Option<&'static [u8]> {
    let total = HISTORY_COUNT.load(Ordering::Relaxed);
    if offset >= total.min(HISTORY_SIZE) { return None; }
    let idx = (total - 1 - offset) % HISTORY_SIZE;
    let slot = &HISTORY[idx];
    let mut end = 0;
    while end < MAX_LINE && slot[end] != 0 { end += 1; }
    Some(&slot[..end])
}

fn parse_u64(s: &str) -> Option<u64> {
    s.parse::<u64>().ok()
}

fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(s, 16).ok()
}

fn parse_hex_byte(s: &str) -> Option<u8> {
    if s.len() != 2 { return None; }
    u8::from_str_radix(s, 16).ok()
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

fn print_uptime_hms() {
    let secs = read_cntpct() / read_cntfrq();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    println!("Uptime: {:02}:{:02}:{:02}", h, m, s);
}

fn strlen_at(addr: u64) -> usize {
    let mut n = 0;
    while n < 4096 {
        let byte = unsafe { core::ptr::read_volatile((addr + n as u64) as *const u8) };
        if byte == 0 { break; }
        n += 1;
    }
    n
}

// ============================================================
// Command table (for tab completion)
// ============================================================

const COMMANDS: &[&str] = &[
    "help", "clear", "ticks", "uptime", "time", "threads", "tasks", "el", "regs", "irqs",
    "mmu", "ptdump", "spawn", "spawnmany", "mem", "dump", "write", "memset", "strlen",
    "alloc", "heap", "vmtest", "vmdemo", "vminfo", "tlb", "ttbr", "mkfs", "touch",
    "writef", "append", "cat", "ls", "rm", "stat", "fsinfo", "cp", "mv", "head", "wc",
    "echo", "sleep", "history", "spam", "sh", "halt", "shutdown", "reboot", "panic",
];

/// Commands that take a filename as their first argument
const FILE_COMMANDS: &[&str] = &[
    "cat", "rm", "stat", "writef", "append", "head", "wc", "cp", "mv", "sh",
];

/// Tab completion. Given the current buffer contents, return a completed version.
/// Called when Tab (0x09) is pressed.
/// Returns (new_buf, new_len, new_cursor).
fn tab_complete(buf: &[u8], len: usize) -> (usize, usize) {
    // Split into first word (command) and rest
    let mut first_space = len;
    let mut i = 0;
    while i < len {
        if buf[i] == b' ' {
            first_space = i;
            break;
        }
        i += 1;
    }

    if first_space == len {
        // Only one word so far — complete command name
        let prefix = unsafe { core::str::from_utf8_unchecked(&buf[..len]) };
        if prefix.is_empty() { return (len, len); }

        // Find all matches
        let mut matches: [&str; 8] = [""; 8];
        let mut match_count = 0;
        for cmd in COMMANDS {
            if cmd.starts_with(prefix) {
                if match_count < 8 {
                    matches[match_count] = cmd;
                    match_count += 1;
                }
            }
        }

        if match_count == 0 {
            // No match — bell
            Uart.putc(0x07);
            return (len, len);
        } else if match_count == 1 {
            // Unique match — replace the buffer
            let m = matches[0];
            let bytes = m.as_bytes();
            let new_len = bytes.len();
            // Copy to buffer... but we can't write to buf here since it's &[u8]
            // We'll do the writing in the caller instead.
            // Return new_len as a signal; the caller will redraw.
            // For simplicity, we print the completion directly and return the new length.
            // Move cursor back to start of word
            let mut j = 0;
            while j < len {
                print!("\x1b[D");
                j += 1;
            }
            // Print the completion
            let mut k = 0;
            while k < new_len {
                Uart.putc(bytes[k]);
                k += 1;
            }
            return (new_len, new_len);
        } else {
            // Multiple matches — print them below
            println!();
            let mut j = 0;
            while j < match_count {
                print!("{}  ", matches[j]);
                j += 1;
            }
            println!();
            // Redraw prompt + current buffer
            print!("{}adityaos>{} ", CYAN, RESET);
            let mut k = 0;
            while k < len {
                Uart.putc(buf[k]);
                k += 1;
            }
            return (len, len);
        }
    } else {
        // We have "command arg..." — complete filename if command expects one
        let cmd = unsafe { core::str::from_utf8_unchecked(&buf[..first_space]) };
        if !FILE_COMMANDS.contains(&cmd) {
            Uart.putc(0x07);
            return (len, len);
        }

        // Find the current partial argument (after last space)
        let mut arg_start = first_space + 1;
        let mut j = first_space + 1;
        while j < len {
            if buf[j] == b' ' {
                arg_start = j + 1;
            }
            j += 1;
        }

        // Only complete the first arg for now
        if arg_start != first_space + 1 {
            Uart.putc(0x07);
            return (len, len);
        }

        let partial = unsafe { core::str::from_utf8_unchecked(&buf[arg_start..len]) };

        // Find matching filenames
        let mut found_first = false;
        let mut unique_match = false;
        let mut match_name: [u8; 32] = [0; 32];
        let mut match_name_len = 0;
        let mut match_count = 0;

        unsafe {
            for k in 0..FS_MAX_FILES {
                if FS.inodes[k].node_type == FsNodeType::Empty { continue; }
                let name_len = FS.inodes[k].name_len;
                let name_bytes = &FS.inodes[k].name[..name_len];
                let name_str = core::str::from_utf8_unchecked(name_bytes);
                if name_str.starts_with(partial) {
                    match_count += 1;
                    if !found_first {
                        found_first = true;
                        match_name_len = name_len;
                        let mut m = 0;
                        while m < name_len && m < 32 {
                            match_name[m] = name_bytes[m];
                            m += 1;
                        }
                    } else {
                        unique_match = false;
                    }
                }
            }
        }

        if match_count == 0 {
            Uart.putc(0x07);
            return (len, len);
        } else if match_count == 1 && unique_match {
            // Print the completion (rest of the name)
            let to_print = match_name_len - partial.len();
            let mut m = partial.len();
            while m < match_name_len {
                Uart.putc(match_name[m]);
                m += 1;
            }
            let _ = to_print;
            return (match_name_len + arg_start, match_name_len + arg_start);
        } else if match_count == 1 {
            // Found exactly one match (unique_match was false because we didn't loop a second time)
            let to_print = match_name_len - partial.len();
            let mut m = partial.len();
            while m < match_name_len {
                Uart.putc(match_name[m]);
                m += 1;
            }
            let _ = to_print;
            return (match_name_len + arg_start, match_name_len + arg_start);
        } else {
            // Multiple matches — bell only, to keep it simple
            Uart.putc(0x07);
            return (len, len);
        }
    }
}

// ============================================================
// Script runner
// ============================================================

fn run_script(name: &str) -> Result<(), &'static str> {
    if !fs_is_formatted() {
        return Err("filesystem not formatted");
    }
    let idx = match fs_find(name) {
        Some(i) => i,
        None => return Err("script not found"),
    };

    let size = unsafe { FS.inodes[idx].size };
    if size == 0 {
        return Err("empty script");
    }
    if size > 2048 {
        return Err("script too large");
    }

    let mut buf = [0u8; 2048];
    match fs_read(idx, &mut buf) {
        Ok(n) => {
            let script = unsafe { core::str::from_utf8_unchecked(&buf[..n]) };
            for line in script.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() { continue; }
                if trimmed.starts_with('#') { continue; }
                println!("{}>>> {}{}", YELLOW, trimmed, RESET);
                handle_command(trimmed);
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

// ============================================================
// Command dispatch
// ============================================================

fn handle_command(line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() { return; }
    history_push(trimmed);

    let mut parts = trimmed.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("").trim();

    match cmd {
        "help" => {
            println!("{}AdityaOS shell commands:{}", BOLD, RESET);
            println!("  {}=== system ==={}", YELLOW, RESET);
            println!("  help                     - show this message");
            println!("  clear                    - clear screen");
            println!("  ticks                    - timer tick count");
            println!("  uptime                   - uptime as HH:MM:SS");
            println!("  el                       - exception level");
            println!("  regs                     - CPU state");
            println!("  irqs                     - interrupt counts");
            println!("  mmu                      - MMU registers");
            println!("  ptdump                   - kernel page table dump");
            println!("  {}=== threads ==={}", YELLOW, RESET);
            println!("  threads                  - thread/scheduler info");
            println!("  tasks                    - thread table");
            println!("  spawn                    - spawn lifecycle thread");
            println!("  spawnmany <n>            - spawn N threads");
            println!("  {}=== memory ==={}", YELLOW, RESET);
            println!("  mem <addr>               - dump 128 bytes");
            println!("  dump <addr> <len>        - dump len bytes");
            println!("  write <addr> <hexbytes>  - write hex bytes");
            println!("  memset <addr> <byte> <len> - fill memory");
            println!("  strlen <addr>            - string length");
            println!("  alloc <size>             - allocate memory");
            println!("  heap                     - heap statistics");
            println!("  {}=== virtualization ==={}", YELLOW, RESET);
            println!("  vmtest                   - setup isolation mappings");
            println!("  vmdemo                   - 2 threads, same VA, different data");
            println!("  vminfo                   - show last vmdemo results");
            println!("  tlb <idx> <vaddr> <paddr> - map vaddr->paddr for thread idx");
            println!("  ttbr <idx>               - show thread's TTBR0");
            println!("  {}=== filesystem ==={}", YELLOW, RESET);
            println!("  mkfs                     - format RAM filesystem");
            println!("  touch <name>             - create empty file");
            println!("  writef <name> <text>     - write text to file");
            println!("  append <name> <text>     - append text to file");
            println!("  cat <name>               - print file contents");
            println!("  head <name> [n]          - print first n lines (default 5)");
            println!("  wc <name>                - count lines, words, bytes");
            println!("  ls                       - list files");
            println!("  cp <src> <dst>           - copy file");
            println!("  mv <src> <dst>           - rename file");
            println!("  rm <name>                - delete file");
            println!("  stat <name>              - file info");
            println!("  fsinfo                   - filesystem statistics");
            println!("  sh <script>              - run a script from the filesystem");
            println!("  {}=== terminal shortcuts ==={}", YELLOW, RESET);
            println!("  Tab                      - complete command or filename");
            println!("  Up/Down                  - history navigation");
            println!("  Ctrl+A / Ctrl+E          - move cursor to start/end");
            println!("  Ctrl+U                   - clear line");
            println!("  Ctrl+W                   - delete last word");
            println!("  Ctrl+L                   - clear screen");
            println!("  Ctrl+C                   - cancel line");
            println!("  {}=== misc ==={}", YELLOW, RESET);
            println!("  echo <text>              - print text");
            println!("  sleep <ms>               - busy-wait sleep");
            println!("  history                  - command history");
            println!("  spam <n>                 - print N long lines");
            println!("  halt                     - stop the CPU");
            println!("  shutdown                 - power off via PSCI");
            println!("  reboot                   - reset via PSCI");
            println!("  panic                    - trigger panic");
        }
        "clear" => clear_screen(),
        "ticks" => println!("Ticks: {}", TICKS.load(Ordering::Relaxed)),
        "uptime" | "time" => print_uptime_hms(),
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
            println!("Idx  State    WakeAt             SP                 TTBR0");
            println!("---  -------  -----------------  -----------------  -----------------");
            unsafe {
                for i in 0..MAX_THREADS {
                    let s = STATE[i];
                    if s == ThreadState::Unused { continue; }
                    println!("{:>3}  {:<7}  {:<17}  {:#018x}  {:#018x}",
                        i, state_name(s), CTX[i].wake_at, CTX[i].sp, tt_thread_l0_phys(i));
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
                println!("{}Usage: sleep <ms>{}", RED, RESET);
            }
        }
        "mem" => {
            if let Some(addr) = parse_hex(args) {
                dump_hex_ascii(addr, 128);
            } else {
                println!("{}Usage: mem <hex address>{}", RED, RESET);
            }
        }
        "dump" => {
            let mut parts2 = args.split_whitespace();
            let addr_str = parts2.next().unwrap_or("");
            let len_str = parts2.next().unwrap_or("");
            let addr = parse_hex(addr_str);
            let len = parse_u64(len_str);
            match (addr, len) {
                (Some(a), Some(l)) if l > 0 && l <= 4096 => dump_hex_ascii(a, l as usize),
                _ => println!("{}Usage: dump <hex addr> <len>{}", RED, RESET),
            }
        }
        "write" => {
            let mut parts2 = args.split_whitespace();
            let addr_str = parts2.next().unwrap_or("");
            let bytes_str = parts2.next().unwrap_or("");
            match parse_hex(addr_str) {
                Some(addr) => {
                    if bytes_str.is_empty() || bytes_str.len() % 2 != 0 {
                        println!("{}Usage: write <hex addr> <hexbytes>{}", RED, RESET);
                    } else {
                        let mut i = 0;
                        let mut ok = true;
                        let bytes = bytes_str.as_bytes();
                        let mut written = 0;
                        while i < bytes.len() {
                            let pair = unsafe {
                                core::str::from_utf8_unchecked(&bytes[i..i+2])
                            };
                            match parse_hex_byte(pair) {
                                Some(b) => {
                                    unsafe {
                                        core::ptr::write_volatile(
                                            (addr + written as u64) as *mut u8, b);
                                    }
                                    written += 1;
                                }
                                None => { ok = false; break; }
                            }
                            i += 2;
                        }
                        if ok {
                            println!("{}Wrote {} bytes at {:#x}{}", GREEN, written, addr, RESET);
                        } else {
                            println!("{}write: invalid hex bytes{}", RED, RESET);
                        }
                    }
                }
                None => println!("{}Usage: write <hex addr> <hexbytes>{}", RED, RESET),
            }
        }
        "memset" => {
            let mut parts2 = args.split_whitespace();
            let addr_str = parts2.next().unwrap_or("");
            let byte_str = parts2.next().unwrap_or("");
            let len_str = parts2.next().unwrap_or("");
            let addr = parse_hex(addr_str);
            let byte = parse_hex_byte(byte_str);
            let len = parse_u64(len_str);
            match (addr, byte, len) {
                (Some(a), Some(b), Some(l)) if l > 0 && l <= 65536 => {
                    let mut i = 0u64;
                    while i < l {
                        unsafe {
                            core::ptr::write_volatile((a + i) as *mut u8, b);
                        }
                        i += 1;
                    }
                    println!("{}Filled {} bytes at {:#x} with 0x{:02x}{}", GREEN, l, a, b, RESET);
                }
                _ => println!("{}Usage: memset <hex addr> <hex byte> <len>{}", RED, RESET),
            }
        }
        "strlen" => {
            if let Some(addr) = parse_hex(args) {
                let n = strlen_at(addr);
                println!("Length: {} bytes", n);
            } else {
                println!("{}Usage: strlen <hex addr>{}", RED, RESET);
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
                Some(idx) => println!("{}Spawned lifecycle thread in slot {}{}", GREEN, idx, RESET),
                None => println!("{}No free spawn slots.{}", RED, RESET),
            }
        }
        "spawnmany" => {
            if let Some(n) = parse_u64(args) {
                let n = n.min(10) as usize;
                let count = spawn_many(n);
                println!("{}Spawned {} threads.{}", GREEN, count, RESET);
            } else {
                println!("{}Usage: spawnmany <n>{}", RED, RESET);
            }
        }
        "spam" => {
            if let Some(n) = parse_u64(args) {
                let n = n.min(200) as usize;
                for i in 1..=n {
                    println!("[spam {:>3}/{}] AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        i, n);
                }
                println!("{}[spam done]{}", GREEN, RESET);
            } else {
                println!("{}Usage: spam <n>{}", RED, RESET);
            }
        }
        "alloc" => {
            if let Some(size) = parse_u64(args) {
                let size = size as usize;
                if size == 0 || size > HEAP_SIZE {
                    println!("{}alloc: size must be 1..{}{}", RED, HEAP_SIZE, RESET);
                } else {
                    let ptr = kmalloc(size, 16);
                    if ptr.is_null() {
                        println!("{}alloc: out of memory{}", RED, RESET);
                    } else {
                        println!("{}alloc: {} bytes at {:#018x}{}", GREEN, size, ptr as usize, RESET);
                    }
                }
            } else {
                println!("{}Usage: alloc <bytes>{}", RED, RESET);
            }
        }
        "heap" => {
            let used = heap_used();
            let total = HEAP_SIZE;
            let free = total - used;
            println!("Heap start: {:#018x}", HEAP_START);
            println!("Heap end:   {:#018x}", HEAP_END);
            println!("Heap next:  {:#018x}", HEAP_NEXT.load(Ordering::Relaxed));
            println!("Total:      {} bytes", total);
            println!("Used:       {} bytes", used);
            println!("Free:       {} bytes", free);
            println!("Allocs:     {}", HEAP_ALLOCS.load(Ordering::Relaxed));
        }
        "ptdump" => {
            let l0 = tt_kernel_l0_phys();
            println!("Kernel TT_L0: {:#018x}", l0);
            unsafe {
                let l0_ptr = l0 as *const u64;
                let l0_entry0 = core::ptr::read_volatile(l0_ptr);
                println!("Kernel L0[0] = {:#018x}", l0_entry0);
                let l1 = (l0_entry0 & !0xFFF) as *const u64;
                println!("Kernel L1 base = {:#018x}", l1 as usize);
                let l1_0 = core::ptr::read_volatile(l1);
                let l1_1 = core::ptr::read_volatile(l1.add(1));
                println!("Kernel L1[0] = {:#018x}", l1_0);
                println!("Kernel L1[1] = {:#018x}", l1_1);
            }
        }
        "mmu" => {
            println!("MAIR_EL1:  {:#018x}", read_mair());
            println!("TCR_EL1:   {:#018x}", read_tcr());
            println!("TTBR0_EL1: {:#018x}", read_ttbr0());
            println!("SCTLR_EL1: {:#018x}", read_sctlr());
            let sctlr = read_sctlr();
            if (sctlr & 1) == 0 {
                println!("{}MMU: OFF{}", RED, RESET);
            } else {
                println!("{}MMU: ON{}", GREEN, RESET);
            }
        }
        "ttbr" => {
            if let Some(idx) = parse_u64(args) {
                let i = idx as usize;
                if i < MAX_THREADS {
                    println!("Thread {} TTBR0 = {:#018x}", i, tt_thread_l0_phys(i));
                } else {
                    println!("{}Thread index must be 0..{}{}", RED, MAX_THREADS, RESET);
                }
            } else {
                for i in 0..MAX_THREADS {
                    println!("Thread {} TTBR0 = {:#018x}", i, tt_thread_l0_phys(i));
                }
            }
        }
        "tlb" => {
            let mut parts2 = args.split_whitespace();
            let idx_s = parts2.next().unwrap_or("");
            let va_s = parts2.next().unwrap_or("");
            let pa_s = parts2.next().unwrap_or("");
            match (parse_u64(idx_s), parse_hex(va_s), parse_hex(pa_s)) {
                (Some(i), Some(va), Some(pa)) if (i as usize) < MAX_THREADS => {
                    if thread_map_page(i as usize, va, pa) {
                        println!("{}Mapped thread {} vaddr {:#x} -> paddr {:#x}{}", GREEN, i, va, pa, RESET);
                    } else {
                        println!("{}tlb: mapping failed{}", RED, RESET);
                    }
                }
                _ => println!("{}Usage: tlb <thread_idx> <vaddr> <paddr>{}", RED, RESET),
            }
        }
        "vmtest" => {
            let p1 = kmalloc(4096, 4096);
            let p2 = kmalloc(4096, 4096);
            if p1.is_null() || p2.is_null() {
                println!("{}vmtest: out of memory{}", RED, RESET);
            } else {
                let pa1 = p1 as u64;
                let pa2 = p2 as u64;
                println!("vmtest: allocated physical pages at {:#x} and {:#x}", pa1, pa2);

                let va: u64 = 0x80_0000_0000 + 0x10_0000;

                if !thread_map_page(0, va, pa1) || !thread_map_page(1, va, pa2) {
                    println!("{}vmtest: mapping failed{}", RED, RESET);
                } else {
                    println!("{}vmtest: mapped VA {:#x}{}", GREEN, va, RESET);
                    println!("  thread 0 -> physical {:#x}", pa1);
                    println!("  thread 1 -> physical {:#x}", pa2);
                }
            }
        }
        "vmdemo" => {
            let mut spawned = 0;
            for _ in 0..2 {
                match spawn_vm_demo() {
                    Some(_) => spawned += 1,
                    None => break,
                }
            }
            println!("{}vmdemo: spawned {} isolation threads{}", GREEN, spawned, RESET);
            println!("  Both write to VA {:#x} with different patterns.", DEMO_VA);
            println!("  After a few seconds, run 'vminfo' to see results.");
        }
        "vminfo" => {
            println!("VM demo results:");
            println!("Idx  Physical");
            println!("---  -----------------");
            unsafe {
                for i in 0..MAX_THREADS {
                    if DEMO_PHYS[i] == 0 { continue; }
                    println!("{:>3}  {:#018x}", i, DEMO_PHYS[i]);
                }
            }
        }
        "mkfs" => {
            fs_format();
            println!("{}Formatted RAM filesystem ({} blocks x {} bytes = {} bytes){}",
                GREEN, FS_MAX_BLOCKS, FS_BLOCK_SIZE, FS_MAX_BLOCKS * FS_BLOCK_SIZE, RESET);
        }
        "touch" => {
            if args.is_empty() {
                println!("{}Usage: touch <name>{}", RED, RESET);
            } else {
                match fs_create(args) {
                    Ok(_) => println!("{}Created: {}{}", GREEN, args, RESET),
                    Err(e) => println!("{}touch: {}{}", RED, e, RESET),
                }
            }
        }
        "writef" => {
            let mut parts2 = args.splitn(2, ' ');
            let name = parts2.next().unwrap_or("");
            let text = parts2.next().unwrap_or("");
            if name.is_empty() {
                println!("{}Usage: writef <name> <text>{}", RED, RESET);
            } else {
                match fs_find(name) {
                    Some(idx) => {
                        match fs_write(idx, text.as_bytes()) {
                            Ok(n) => println!("{}Wrote {} bytes to {}{}", GREEN, n, name, RESET),
                            Err(e) => println!("{}writef: {}{}", RED, e, RESET),
                        }
                    }
                    None => {
                        match fs_create(name) {
                            Ok(idx) => match fs_write(idx, text.as_bytes()) {
                                Ok(n) => println!("{}Created {} and wrote {} bytes{}", GREEN, name, n, RESET),
                                Err(e) => println!("{}writef: {}{}", RED, e, RESET),
                            },
                            Err(e) => println!("{}writef: {}{}", RED, e, RESET),
                        }
                    }
                }
            }
        }
        "append" => {
            let mut parts2 = args.splitn(2, ' ');
            let name = parts2.next().unwrap_or("");
            let text = parts2.next().unwrap_or("");
            if name.is_empty() {
                println!("{}Usage: append <name> <text>{}", RED, RESET);
            } else {
                match fs_find(name) {
                    Some(idx) => {
                        match fs_append(idx, text.as_bytes()) {
                            Ok(n) => println!("{}Appended {} bytes to {}{}", GREEN, n, name, RESET),
                            Err(e) => println!("{}append: {}{}", RED, e, RESET),
                        }
                    }
                    None => println!("{}append: {}: no such file{}", RED, name, RESET),
                }
            }
        }
        "cat" => {
            if args.is_empty() {
                println!("{}Usage: cat <name>{}", RED, RESET);
            } else {
                match fs_find(args) {
                    Some(idx) => {
                        let size = unsafe { FS.inodes[idx].size };
                        if size == 0 {
                            println!("(empty file)");
                        } else {
                            let mut buf = [0u8; FS_BLOCK_SIZE * FS_MAX_BLOCKS_PER_FILE];
                            match fs_read(idx, &mut buf) {
                                Ok(n) => {
                                    match core::str::from_utf8(&buf[..n]) {
                                        Ok(s) => println!("{}", s),
                                        Err(_) => {
                                            println!("(binary file, {} bytes)", n);
                                        }
                                    }
                                }
                                Err(e) => println!("{}cat: {}{}", RED, e, RESET),
                            }
                        }
                    }
                    None => println!("{}cat: {}: no such file{}", RED, args, RESET),
                }
            }
        }
        "head" => {
            let mut parts2 = args.split_whitespace();
            let name = parts2.next().unwrap_or("");
            let n_str = parts2.next().unwrap_or("5");
            if name.is_empty() {
                println!("{}Usage: head <name> [n]{}", RED, RESET);
            } else {
                let n = parse_u64(n_str).unwrap_or(5) as usize;
                match fs_find(name) {
                    Some(idx) => {
                        let size = unsafe { FS.inodes[idx].size };
                        if size == 0 {
                            println!("(empty file)");
                        } else {
                            let mut buf = [0u8; FS_BLOCK_SIZE * FS_MAX_BLOCKS_PER_FILE];
                            match fs_read(idx, &mut buf) {
                                Ok(len) => {
                                    match core::str::from_utf8(&buf[..len]) {
                                        Ok(s) => {
                                            let mut count = 0;
                                            for line in s.lines() {
                                                if count >= n { break; }
                                                println!("{}", line);
                                                count += 1;
                                            }
                                        }
                                        Err(_) => println!("{}head: binary file{}", RED, RESET),
                                    }
                                }
                                Err(e) => println!("{}head: {}{}", RED, e, RESET),
                            }
                        }
                    }
                    None => println!("{}head: {}: no such file{}", RED, name, RESET),
                }
            }
        }
        "wc" => {
            if args.is_empty() {
                println!("{}Usage: wc <name>{}", RED, RESET);
            } else {
                match fs_find(args) {
                    Some(idx) => {
                        let size = unsafe { FS.inodes[idx].size };
                        if size == 0 {
                            println!("  0   0   0 {}", args);
                        } else {
                            let mut buf = [0u8; FS_BLOCK_SIZE * FS_MAX_BLOCKS_PER_FILE];
                            match fs_read(idx, &mut buf) {
                                Ok(len) => {
                                    match core::str::from_utf8(&buf[..len]) {
                                        Ok(s) => {
                                            let lines = s.lines().count();
                                            let words = s.split_whitespace().count();
                                            let bytes = len;
                                            println!("{:>3} {:>3} {:>3} {}", lines, words, bytes, args);
                                        }
                                        Err(_) => println!("wc: binary file"),
                                    }
                                }
                                Err(e) => println!("{}wc: {}{}", RED, e, RESET),
                            }
                        }
                    }
                    None => println!("{}wc: {}: no such file{}", RED, args, RESET),
                }
            }
        }
        "ls" => {
            if !fs_is_formatted() {
                println!("{}Filesystem not formatted. Run 'mkfs' first.{}", RED, RESET);
            } else {
                let count = fs_count_files();
                if count == 0 {
                    println!("(empty directory)");
                } else {
                    println!("{}NAME                             SIZE{}", BOLD, RESET);
                    println!("------------------------------   ------");
                    unsafe {
                        for i in 0..FS_MAX_FILES {
                            if FS.inodes[i].node_type == FsNodeType::Empty { continue; }
                            let mut line = [b' '; 40];
                            for j in 0..FS.inodes[i].name_len {
                                line[j] = FS.inodes[i].name[j];
                            }
                            let name_str = core::str::from_utf8_unchecked(&line[..30]);
                            print!("{}", BLUE);
                            print!("{:<30}", name_str);
                            print!("{}", RESET);
                            println!("   {} bytes", FS.inodes[i].size);
                        }
                    }
                }
            }
        }
        "cp" => {
            let mut parts2 = args.split_whitespace();
            let src = parts2.next().unwrap_or("");
            let dst = parts2.next().unwrap_or("");
            if src.is_empty() || dst.is_empty() {
                println!("{}Usage: cp <src> <dst>{}", RED, RESET);
            } else {
                match fs_find(src) {
                    Some(src_idx) => {
                        if fs_find(dst).is_some() {
                            println!("{}cp: {}: already exists{}", RED, dst, RESET);
                        } else {
                            match fs_create(dst) {
                                Ok(dst_idx) => {
                                    let mut buf = [0u8; FS_BLOCK_SIZE * FS_MAX_BLOCKS_PER_FILE];
                                    let size = unsafe { FS.inodes[src_idx].size };
                                    match fs_read(src_idx, &mut buf) {
                                        Ok(n) => {
                                            match fs_write(dst_idx, &buf[..n]) {
                                                Ok(_) => println!("{}Copied {} -> {} ({} bytes){}", GREEN, src, dst, size, RESET),
                                                Err(e) => println!("{}cp: {}{}", RED, e, RESET),
                                            }
                                        }
                                        Err(e) => println!("{}cp: {}{}", RED, e, RESET),
                                    }
                                }
                                Err(e) => println!("{}cp: {}{}", RED, e, RESET),
                            }
                        }
                    }
                    None => println!("{}cp: {}: no such file{}", RED, src, RESET),
                }
            }
        }
        "mv" => {
            let mut parts2 = args.split_whitespace();
            let src = parts2.next().unwrap_or("");
            let dst = parts2.next().unwrap_or("");
            if src.is_empty() || dst.is_empty() {
                println!("{}Usage: mv <src> <dst>{}", RED, RESET);
            } else {
                match fs_find(src) {
                    Some(idx) => {
                        if fs_find(dst).is_some() {
                            println!("{}mv: {}: already exists{}", RED, dst, RESET);
                        } else {
                            let (bytes, n) = fs_name_to_bytes(dst);
                            unsafe {
                                FS.inodes[idx].name = bytes;
                                FS.inodes[idx].name_len = n;
                            }
                            println!("{}Renamed {} -> {}{}", GREEN, src, dst, RESET);
                        }
                    }
                    None => println!("{}mv: {}: no such file{}", RED, src, RESET),
                }
            }
        }
        "rm" => {
            if args.is_empty() {
                println!("{}Usage: rm <name>{}", RED, RESET);
            } else {
                match fs_delete(args) {
                    Ok(_) => println!("{}Removed: {}{}", GREEN, args, RESET),
                    Err(e) => println!("{}rm: {}{}", RED, e, RESET),
                }
            }
        }
        "stat" => {
            if args.is_empty() {
                println!("{}Usage: stat <name>{}", RED, RESET);
            } else {
                match fs_find(args) {
                    Some(idx) => unsafe {
                        let inode = &FS.inodes[idx];
                        let mut line = [0u8; FS_MAX_NAME];
                        for j in 0..inode.name_len {
                            line[j] = inode.name[j];
                        }
                        let name_str = core::str::from_utf8_unchecked(&line[..inode.name_len]);
                        println!("{}File: {}{}", BOLD, name_str, RESET);
                        println!("  type:   file");
                        println!("  size:   {} bytes", inode.size);
                        println!("  blocks: {}", inode.block_count);
                        for k in 0..inode.block_count {
                            println!("    block[{}] -> #{}", k, inode.blocks[k]);
                        }
                    },
                    None => println!("{}stat: {}: no such file{}", RED, args, RESET),
                }
            }
        }
        "fsinfo" => {
            if !fs_is_formatted() {
                println!("{}Filesystem not formatted. Run 'mkfs' first.{}", RED, RESET);
            } else {
                println!("{}Filesystem: RAM-backed (in BSS){}", BOLD, RESET);
                println!("  magic:        {:#010x}", unsafe { FS.magic });
                println!("  block size:   {} bytes", FS_BLOCK_SIZE);
                println!("  total blocks: {}", FS_MAX_BLOCKS);
                println!("  used blocks:  {}", fs_count_used_blocks());
                println!("  free blocks:  {}", FS_MAX_BLOCKS - fs_count_used_blocks());
                println!("  total bytes:  {}", FS_MAX_BLOCKS * FS_BLOCK_SIZE);
                println!("  max inodes:   {}", FS_MAX_FILES);
                println!("  files:        {}", fs_count_files());
            }
        }
        "sh" => {
            if args.is_empty() {
                println!("{}Usage: sh <script>{}", RED, RESET);
            } else {
                match run_script(args) {
                    Ok(_) => println!("{}Script '{}' finished.{}", GREEN, args, RESET),
                    Err(e) => println!("{}sh: {}{}", RED, e, RESET),
                }
            }
        }
        "halt" => {
            println!("Halting (CPU stopped). Use 'shutdown' to power off, or close QEMU.");
            sleep_ms(200);
            unsafe { core::arch::asm!("msr daifset, #2", options(nostack, nomem)); }
            loop { unsafe { core::arch::asm!("wfi", options(nostack, nomem)); } }
        }
        "shutdown" => {
            println!("Shutting down via PSCI...");
            sleep_ms(200);
            unsafe {
                core::arch::asm!(
                    "movz x0, #0x0008",
                    "movk x0, #0x8400, lsl #16",
                    "hvc #0",
                    out("x0") _,
                    options(nostack, nomem)
                );
            }
            loop { unsafe { core::arch::asm!("wfi", options(nostack, nomem)); } }
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
            println!("{}Unknown command: '{}'{}", RED, cmd, RESET);
            println!("Type 'help' for a list of commands.");
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum TermState {
    Normal,
    EscapeSeen,
    BracketSeen,
}

extern "C" fn shell_thread() -> ! {
    let mut buf = [0u8; MAX_LINE];
    let mut len: usize = 0;
    let mut cursor: usize = 0;
    let mut term_state = TermState::Normal;
    let mut hist_offset: usize = 0;
    let mut hist_browsing = false;
    let mut saved_line = [0u8; MAX_LINE];
    let mut saved_len: usize = 0;

    thread_sleep(5);

    println!();
    println!("{}================================={}", CYAN, RESET);
    println!("{}     AdityaOS Shell v1.16{}", BOLD, RESET);
    println!("{}   Colors · Tabs · Scripts{}", CYAN, RESET);
    println!("{}================================={}", CYAN, RESET);
    println!();

    shell_prompt();

    loop {
        let c = match uart_try_getc() {
            Some(b) => b,
            None => {
                thread_sleep(1);
                continue;
            }
        };

        match term_state {
            TermState::EscapeSeen => {
                if c == b'[' {
                    term_state = TermState::BracketSeen;
                } else {
                    term_state = TermState::Normal;
                }
                continue;
            }
            TermState::BracketSeen => {
                match c {
                    b'A' => {
                        if !hist_browsing {
                            saved_len = len;
                            let mut i = 0;
                            while i < len {
                                saved_line[i] = buf[i];
                                i += 1;
                            }
                            hist_browsing = true;
                            hist_offset = 0;
                        } else {
                            hist_offset += 1;
                        }
                        unsafe {
                            match history_get(hist_offset) {
                                Some(entry) => {
                                    len = entry.len();
                                    let mut i = 0;
                                    while i < len {
                                        buf[i] = entry[i];
                                        i += 1;
                                    }
                                    cursor = len;
                                    redraw_line(&buf, len, cursor);
                                }
                                None => {
                                    hist_offset -= 1;
                                }
                            }
                        }
                    }
                    b'B' => {
                        if hist_browsing {
                            if hist_offset == 0 {
                                len = saved_len;
                                let mut i = 0;
                                while i < len {
                                    buf[i] = saved_line[i];
                                    i += 1;
                                }
                                cursor = len;
                                hist_browsing = false;
                                redraw_line(&buf, len, cursor);
                            } else {
                                hist_offset -= 1;
                                unsafe {
                                    match history_get(hist_offset) {
                                        Some(entry) => {
                                            len = entry.len();
                                            let mut i = 0;
                                            while i < len {
                                                buf[i] = entry[i];
                                                i += 1;
                                            }
                                            cursor = len;
                                            redraw_line(&buf, len, cursor);
                                        }
                                        None => {}
                                    }
                                }
                            }
                        }
                    }
                    b'C' => {
                        if cursor < len {
                            cursor += 1;
                            print!("\x1b[C");
                        }
                    }
                    b'D' => {
                        if cursor > 0 {
                            cursor -= 1;
                            print!("\x1b[D");
                        }
                    }
                    _ => {}
                }
                term_state = TermState::Normal;
                continue;
            }
            TermState::Normal => {}
        }

        match c {
            0x1B => {
                term_state = TermState::EscapeSeen;
            }
            0x09 => {
                // Tab — try completion
                let (new_len, new_cursor) = tab_complete(&buf, len);
                len = new_len;
                cursor = new_cursor;
            }
            0x0D | 0x0A => {
                println!();
                if len < MAX_LINE { buf[len] = 0; }
                let line = unsafe { core::str::from_utf8_unchecked(&buf[..len]) };
                handle_command(line);
                len = 0;
                cursor = 0;
                hist_browsing = false;
                hist_offset = 0;
                shell_prompt();
            }
            0x7F | 0x08 => {
                if cursor > 0 {
                    let mut i = cursor - 1;
                    while i < len - 1 {
                        buf[i] = buf[i + 1];
                        i += 1;
                    }
                    len -= 1;
                    cursor -= 1;
                    redraw_line(&buf, len, cursor);
                }
            }
            0x01 => {
                cursor = 0;
                let mut back = len;
                while back > 0 {
                    print!("\x1b[D");
                    back -= 1;
                }
            }
            0x05 => {
                while cursor < len {
                    print!("\x1b[C");
                    cursor += 1;
                }
            }
            0x03 => {
                print!("^C");
                println!();
                len = 0;
                cursor = 0;
                hist_browsing = false;
                hist_offset = 0;
                shell_prompt();
            }
            0x0C => {
                clear_screen();
                shell_prompt();
                let mut i = 0;
                while i < len {
                    Uart.putc(buf[i]);
                    i += 1;
                }
                let mut back = len - cursor;
                while back > 0 {
                    print!("\x1b[D");
                    back -= 1;
                }
            }
            0x15 => {
                len = 0;
                cursor = 0;
                redraw_line(&buf, len, cursor);
            }
            0x17 => {
                if cursor > 0 {
                    let mut end = cursor;
                    while end > 0 && buf[end - 1] == b' ' {
                        end -= 1;
                    }
                    let mut start = end;
                    while start > 0 && buf[start - 1] != b' ' {
                        start -= 1;
                    }
                    let delete_count = cursor - start;
                    let mut i = start;
                    while i + delete_count < len {
                        buf[i] = buf[i + delete_count];
                        i += 1;
                    }
                    len -= delete_count;
                    cursor = start;
                    redraw_line(&buf, len, cursor);
                }
            }
            0x20..=0x7E => {
                if len < MAX_LINE - 1 {
                    if cursor == len {
                        buf[len] = c;
                        len += 1;
                        cursor += 1;
                        Uart.putc(c);
                    } else {
                        let mut i = len;
                        while i > cursor {
                            buf[i] = buf[i - 1];
                            i -= 1;
                        }
                        buf[cursor] = c;
                        len += 1;
                        cursor += 1;
                        redraw_line(&buf, len, cursor);
                    }
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
    println!("{}=== AdityaOS Exception ==={}", RED, RESET);
    println!("  ESR_EL1: {:#018x}", esr);
    println!("  ELR_EL1: {:#018x}", elr);
    println!("  FAR_EL1: {:#018x}", far);
    let ec = (esr >> 26) & 0x3F;
    println!("  EC: {:#x}", ec);
    if ec == 0x24 || ec == 0x25 {
        let dfsc = esr & 0x3F;
        println!("  DFSC: {:#x}", dfsc);
    }
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

                let new_ttbr = tt_thread_l0_phys(nxt);
                if new_ttbr != 0 {
                    set_ttbr0(new_ttbr as u64);
                }

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
    print_slow("       AdityaOS v1.16\n", 3);
    print_slow("   Colors · Tabs · Scripts\n", 3);
    print_slow("=================================\n", 3);
    println!();
    println!("Running at EL{}", current_el());
    println!("Timer frequency: {} Hz", read_cntfrq());
    println!();

    println!("Initializing heap...");
    heap_init();
    println!("  heap: {:#x}..{:#x} ({} MB)",
        HEAP_START, HEAP_END, HEAP_SIZE / (1024 * 1024));

    println!("Building kernel page tables (identity map)...");
    kernel_page_tables_init();
    println!("  Kernel TT_L0 at {:#x}", tt_kernel_l0_phys());

    println!("Building per-thread page tables...");
    thread_page_tables_init();
    println!("  Thread 0 TTBR0 = {:#x}", tt_thread_l0_phys(0));

    println!("Configuring MMU registers...");
    mmu_configure();

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

    println!("Enabling MMU...");
    mmu_enable();
    println!("MMU enabled! SCTLR_EL1 = {:#018x}", read_sctlr());
    println!();

    let ttbr0 = tt_thread_l0_phys(0) as u64;
    if ttbr0 != 0 {
        set_ttbr0(ttbr0);
        println!("Switched to thread 0's page table (TTBR0 = {:#x})", ttbr0);
        println!();
    }

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
