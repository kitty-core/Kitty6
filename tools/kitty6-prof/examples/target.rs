//! A process to try kitty6-prof on without the game: threads that run, queue
//! on a lock, wait on a condition variable and sleep, with known names.
//!
//!   cargo run --release --offline --example target
//!   kitty6-prof --name target.exe --seconds 5 --hz 50

use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

#[inline(never)]
fn spin_work(n: u64) -> u64 {
    let mut x = n;
    for i in 0..20_000_000u64 {
        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(i));
    }
    x
}

#[inline(never)]
fn hold_lock(m: &Mutex<u64>) {
    let mut g = m.lock().unwrap();
    *g = spin_work(*g);
}

#[inline(never)]
fn take_lock(m: &Mutex<u64>) {
    let g = m.lock().unwrap();
    std::hint::black_box(*g);
}

#[inline(never)]
fn wait_job(c: &(Mutex<bool>, Condvar)) {
    let g = c.0.lock().unwrap();
    let _g = c.1.wait_timeout(g, Duration::from_millis(500)).unwrap();
}

#[inline(never)]
fn nap() {
    thread::sleep(Duration::from_millis(20));
}

fn main() {
    let m = Arc::new(Mutex::new(1u64));
    let c = Arc::new((Mutex::new(false), Condvar::new()));
    for i in 0..2 {
        let m = m.clone();
        thread::Builder::new().name(format!("holder-{i}")).spawn(move || loop { hold_lock(&m) }).unwrap();
    }
    for i in 0..3 {
        let m = m.clone();
        thread::Builder::new().name(format!("taker-{i}")).spawn(move || loop { take_lock(&m) }).unwrap();
    }
    {
        let c = c.clone();
        thread::Builder::new().name("waiter".into()).spawn(move || loop { wait_job(&c) }).unwrap();
    }
    thread::Builder::new().name("napper".into()).spawn(|| loop { nap() }).unwrap();
    thread::spawn(|| loop {
        std::hint::black_box(spin_work(3));
    });
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}
