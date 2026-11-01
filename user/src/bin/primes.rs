#![no_std]
#![no_main]

use user::{entry, getpid, println, uptime_ms};

entry!(main);

const MAX: usize = 200_000;
// Variable inicializada: va a .data (ocupa espacio en el archivo).
static mut EJECUCIONES: u64 = 1;
// Va a .bss: ocupa memoria pero no ocupa espacio en el archivo ELF.
static mut SIEVE: [bool; MAX + 1] = [false; MAX + 1];

fn main(args: &str) {
    // Uso: primes [N] [repeticiones]
    let mut it = args.split_whitespace();
    let n = it.next().and_then(|a| a.parse::<usize>().ok()).unwrap_or(50_000).min(MAX);
    let rounds = it.next().and_then(|a| a.parse::<u32>().ok()).unwrap_or(1).clamp(1, 100_000);
    unsafe { EJECUCIONES += 1 };
    let t0 = uptime_ms();
    let sieve = unsafe { &mut *(&raw mut SIEVE) };
    let mut count = 0;
    let mut last = 0;
    for _ in 0..rounds {
        sieve[..=n].fill(false);
        count = 0;
        for i in 2..=n {
            if !sieve[i] {
                count += 1;
                last = i;
                let mut j = i * i;
                while j <= n {
                    sieve[j] = true;
                    j += i;
                }
            }
        }
    }
    println!("[primes pid {}] hasta {}: {} primos (ultimo {}), {} repeticiones en {} ms (.data={})",
        getpid(), n, count, last, rounds, uptime_ms() - t0, unsafe { EJECUCIONES });
}
