# AdityaOS

A small bare-metal operating system for **AArch64 (ARM64)**, written from scratch in Rust and assembly.

AdityaOS boots directly on QEMU's `virt` machine — no Linux, no OS underneath. It runs a preemptive multitasking kernel with virtual memory, per-thread address spaces, a RAM filesystem, and an interactive shell.

Built by **Aditya** from Visakhapatnam, India.

---

## Features

- **Bare-metal AArch64 kernel** — boots on QEMU `virt`, no host OS
- **Custom boot code** — assembly entry point, exception vector table, exception handlers
- **PL011 UART driver** — terminal input and output via MMIO
- **Formatted output** — `print!` and `println!` macros through `core::fmt::Write`
- **Interrupt handling** — GICv2 configuration, ARM Generic Timer, UART RX interrupts
- **Preemptive multitasking** — round-robin scheduler with full context switching
- **Per-thread page tables** — each thread has its own L0 page table
- **MMU enabled** — 4-level page tables (L0 → L1 → L2 → L3), identity-mapped kernel
- **Virtual memory isolation** — two threads can write to the same virtual address and see different data
- **Sleeping threads** — `wfi`-based idle, scheduler wakes on time
- **Dynamic thread spawning** — create threads at runtime, slot reuse on exit
- **Memory allocator** — bump allocator over a reserved heap region
- **RAM filesystem** — inodes, blocks, files (mkfs, touch, writef, cat, ls, cp, mv, rm, stat)
- **Script runner** — execute command files from the filesystem (sh)
- **Interactive shell** — history navigation, tab completion, line editing, ANSI colors
- **Easter eggs** — sudo, coffee, 42, and more
- **ASCII animation** — spinning donut
- **Text game** — moonlander
- **Clean shutdown** — PSCI-based power off and reset

---

## Shell commands

### System

| Command | Description |
|---------|-------------|
| help | Show all commands |
| clear | Clear the screen |
| ticks | Timer tick count |
| uptime | Uptime as HH:MM:SS |
| el | Current exception level |
| regs | CPU state (SP, ELR, SPSR) |
| irqs | Interrupt counts |
| mmu | MMU register values |
| ptdump | Kernel page table dump |

### Threads

| Command | Description |
|---------|-------------|
| threads | Thread and scheduler info |
| tasks | Per-thread state table |
| spawn | Spawn one lifecycle thread |
| spawnmany `<n>` | Spawn up to N threads |

### Memory

| Command | Description |
|---------|-------------|
| mem `<addr>` | Dump 128 bytes |
| dump `<addr> <len>` | Dump len bytes |
| write `<addr> <hex>` | Write hex bytes |
| memset `<addr> <byte> <len>` | Fill memory |
| strlen `<addr>` | String length |
| alloc `<size>` | Allocate memory |
| heap | Heap statistics |

### Virtualization

| Command | Description |
|---------|-------------|
| vmtest | Set up isolation mappings |
| vmdemo | Two threads, same VA, different data |
| vminfo | Show last vmdemo results |
| tlb `<idx> <vaddr> <paddr>` | Map vaddr to paddr for a thread |
| ttbr `<idx>` | Show thread's TTBR0 |

### Filesystem

| Command | Description |
|---------|-------------|
| mkfs | Format RAM filesystem |
| touch `<name>` | Create empty file |
| writef `<name> <text>` | Write text to file |
| append `<name> <text>` | Append text to file |
| cat `<name>` | Print file contents |
| head `<name> [n]` | First n lines |
| wc `<name>` | Line, word, byte count |
| ls | List files |
| cp `<src> <dst>` | Copy file |
| mv `<src> <dst>` | Rename file |
| rm `<name>` | Delete file |
| stat `<name>` | File info |
| fsinfo | Filesystem statistics |
| sh `<script>` | Run a script |

### Terminal shortcuts

| Key | Action |
|-----|--------|
| Tab | Complete command or filename |
| Up / Down | History navigation |
| Ctrl+A / Ctrl+E | Cursor to start/end |
| Ctrl+U | Clear line |
| Ctrl+W | Delete last word |
| Ctrl+L | Clear screen |
| Ctrl+C | Cancel line |

### Fun

| Command | Description |
|---------|-------------|
| donut | ASCII spinning donut |
| moonlander | Text-based landing game |
| sudo | Easter egg |
| coffee | HTTP 418 |
| 42 | The Answer |

### Control

| Command | Description |
|---------|-------------|
| halt | Stop the CPU |
| shutdown | Power off via PSCI |
| reboot | Reset via PSCI |
| panic | Trigger a kernel panic |

---

## Requirements

- Rust nightly with the `aarch64-unknown-none` target
- QEMU 8.x or newer (`qemu-system-aarch64`)
- AArch64 cross-toolchain: `aarch64-linux-gnu-gcc`, `binutils-aarch64-linux-gnu`

### Ubuntu / WSL2

    sudo apt update
    sudo apt install build-essential qemu-system-arm gcc-aarch64-linux-gnu binutils-aarch64-linux-gnu
    rustup target add aarch64-unknown-none

---

## Build and run

    ./build.sh

This compiles the kernel and launches it in QEMU. To exit QEMU, press `Ctrl + A`, release, then `X`.

---

## How it works

### Boot

QEMU loads the kernel at `0x4008_0000` and jumps to `_start` in `boot.S`. The assembly boot code:

1. Sets up the initial stack
2. Zeros the BSS section
3. Installs the exception vector table in `VBAR_EL1`
4. Enables FP/SIMD
5. Jumps to the Rust entry point

### Kernel initialization

`kmain` sets up:

1. **Heap** — 112 MB bump allocator at `0x4800_0000`
2. **Page tables** — kernel identity map, per-thread L0 tables
3. **GIC** — interrupt controller, enables timer and UART IRQs
4. **Timer** — ARM Generic Timer, 100ms tick
5. **Scheduler** — thread table with 8 slots
6. **UART RX interrupts** — interrupt-driven input
7. **MMU** — enables address translation

### Scheduler

Round-robin, preemptive. On every other timer tick (200ms), the current thread's context is saved and the next ready thread's context is restored. If a thread is sleeping, the scheduler skips it until its wake time.

### Virtual memory

Each thread has its own L0 page table. The kernel identity-maps the low 2GB for hardware access. Each thread can add private mappings in the 512GB–1TB range (L0 index 1). Context switches update `TTBR0_EL1` and flush the TLB.

### Filesystem

In-memory filesystem with fixed-size inode and data block tables:

- 32 inode slots
- 64 data blocks of 512 bytes (32 KB total)
- Each file can use up to 4 blocks (2 KB max)
- Unix-style semantics: inode, size, block pointers

Everything lives in BSS — no disk, no persistence across reboot.

---

## Project layout

    AdityaOS/
    ├── .cargo/config.toml      Cargo target and linker flags
    ├── build.rs                Assembles src/boot.S and links it
    ├── linker.ld               Memory layout at 0x40080000
    ├── build.sh                Build and run script
    └── src/
        ├── boot.S              Entry point, exception vectors, SVC handler
        └── main.rs             Kernel: UART, MMU, scheduler, shell, filesystem

---

## Known limitations

- No user mode (EL0) — everything runs at EL1
- No fork/exec — threads run in the kernel's address space
- No persistent storage — filesystem is RAM-only
- Single core — no SMP
- No networking
- Fixed-size filesystem (32 files, 32 KB)

---

## Roadmap

- [ ] User mode (EL0) + syscalls
- [ ] fork / exec
- [ ] Persistent storage (SD card on Raspberry Pi)
- [ ] Port to Raspberry Pi 3B+
- [ ] Multi-core support
- [ ] Networking

---

## Version history

Every major version is tagged:

    git tag                        # list all tags
    git checkout v1.16-clean       # restore a known-good state

| Tag | What it added |
|-----|---------------|
| v1.0-clean | Boot, UART, timer, interrupts |
| v1.1-clean | Shell extensions (regs, mem, history) |
| v1.2-clean | Thread lifecycle + spawn |
| v1.3-clean | 8-slot thread table |
| v1.4-clean | UART RX interrupts |
| v1.5-clean | Sleeping threads (WFI) |
| v1.6-clean | Memory allocator |
| v1.7-clean | Debug command pack |
| v1.8c-clean | MMU enabled |
| v1.9-clean | Per-thread page tables |
| v1.10-clean | VM isolation demo |
| v1.11-clean | RAM filesystem |
| v1.12-clean | Interactive terminal |
| v1.16-clean | Colors, tabs, scripts |
| v1.18-clean | Easter eggs, donut, moonlander |

---

## Credits

The overall approach — boot flow, exception vector layout, GICv2 setup, context switch structure, and page table design — was learned from studying open-source AArch64 kernel projects and the OSDev community:

- [rust-microkernel](https://github.com/bahree/rust-microkernel) by Amit Bahree
- [raspberry-pi-os](https://github.com/s-matyukevich/raspberry-pi-os) by Sergey Matyukevich
- [OSDev Wiki](https://wiki.osdev.org/)
- [ARM Architecture Reference Manual](https://developer.arm.com/documentation/ddi0487/latest/)

The donut animation is based on the classic `donut.c` by Andy Sloane.

All code in this repository was written from scratch by the author.

---

## License

MIT — see [LICENSE](LICENSE) for details.
