//! Planificador expropiativo (round-robin) de hilos de kernel.
//!
//! Cada tarea tiene su propia pila. Cambiar de tarea = guardar los registros
//! "callee-saved" en la pila actual, guardar `rsp`, cargar el `rsp` de la otra
//! tarea y restaurar sus registros. El temporizador llama a `preempt()` en cada
//! tick, así que ninguna tarea necesita ceder la CPU voluntariamente.

use crate::{cpu, gdt, paging, syscall};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::arch::{asm, global_asm};

const STACK_SIZE: usize = 32 * 1024;
/// Valor centinela en el fondo de cada pila: si cambia, la tarea la desbordó.
const CANARY: u64 = 0xDEAD_C0DE_5AFE_57AC;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ready,
    Running,
    Sleeping(u64), // dormida hasta que el reloj llegue a este tick
    WaitInput,     // bloqueada esperando una tecla
    WaitExit(u64), // bloqueada esperando a que termine el proceso con este id
    Dead,
}

struct Task {
    id: u64,
    name: String,
    state: State,
    rsp: usize,          // rsp guardado cuando la tarea NO está en ejecución
    stack: Vec<u8>,      // su pila (vive en el heap); vacía para la tarea inicial
    entry: Option<fn()>, // función que ejecuta
    ticks: u64,          // ticks de CPU consumidos
    idle: bool,          // tarea ociosa: solo corre si no hay nada más que hacer
    // --- procesos de usuario ---
    cr3: u64,         // PML4 propia (0 = tarea de kernel: usa la del kernel)
    kstack_top: u64,  // cima de su pila de kernel (TSS.rsp0 y pila de syscall)
    user_rip: u64,    // dónde empieza a ejecutar en modo usuario
    user_rsp: u64,    // su pila de usuario
    user_arg0: u64,   // valores iniciales de rdi y rsi al entrar a modo usuario
    user_arg1: u64,
    brk_start: u64,   // base del heap de usuario (brk)
    brk_cur: u64,     // fin actual del heap de usuario
}

struct Scheduler {
    tasks: Vec<Box<Task>>, // Box: la dirección de cada Task no cambia al crecer el Vec
    current: u64,          // id de la tarea en ejecución
    next_id: u64,
    kernel_cr3: u64,       // tablas de páginas del kernel (las de Limine)
    exit_codes: Vec<(u64, i64)>, // (id, código) de procesos terminados que nadie ha recogido aún
}

static mut SCHED: Scheduler = Scheduler { tasks: Vec::new(), current: 0, next_id: 1, kernel_cr3: 0, exit_codes: Vec::new() };

fn sched() -> &'static mut Scheduler {
    unsafe { &mut *(&raw mut SCHED) }
}

// ---- Cambio de contexto (ensamblador) ----
//
// switch_context(old_rsp: *mut usize, new_rsp: usize)
//   rdi = dónde guardar el rsp de la tarea saliente, rsi = rsp de la entrante.
//
// thread_trampoline: primer código que ejecuta una tarea NUEVA. Se llega a él con
// un `ret`, y activa las interrupciones (veníamos de un manejador de IRQ, que
// las tiene desactivadas) antes de llamar a thread_start.
global_asm!(
    r#"
    .text
    .global switch_context
    switch_context:
        push rbp
        push rbx
        push r12
        push r13
        push r14
        push r15
        mov [rdi], rsp
        mov rsp, rsi
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbx
        pop rbp
        ret

    .global thread_trampoline
    thread_trampoline:
        sti
        call thread_start
        ud2
    "#
);

extern "C" {
    fn switch_context(old_rsp: *mut usize, new_rsp: usize);
    fn thread_trampoline();
}

// ---- API ----

/// Convierte el hilo actual (el que ejecuta `_start` y el shell) en la tarea 0.
pub fn init() {
    cpu::without_interrupts(|| {
        let s = sched();
        s.tasks.push(Box::new(Task {
            id: 0,
            name: String::from("shell"),
            state: State::Running,
            rsp: 0,
            stack: Vec::new(),
            entry: None,
            ticks: 0,
            idle: false,
            cr3: 0,
            kstack_top: 0,
            user_rip: 0,
            user_rsp: 0,
            user_arg0: 0,
            user_arg1: 0,
            brk_start: 0,
            brk_cur: 0,
        }));
        s.current = 0;
        s.kernel_cr3 = paging::read_cr3() & !0xfff;
    });
    // Tarea ociosa: ocupa la CPU cuando todas las demás están bloqueadas.
    let id = spawn("idle", idle_loop).expect("sin memoria para la tarea idle");
    cpu::without_interrupts(|| {
        if let Some(t) = sched().tasks.iter_mut().find(|t| t.id == id) {
            t.idle = true;
        }
    });
}

fn idle_loop() {
    loop {
        unsafe { asm!("hlt", options(nomem, nostack)) };
    }
}

/// Crea una tarea de kernel que ejecutará `entry`. Devuelve su id, o `None` si no hay memoria.
pub fn spawn(name: &str, entry: fn()) -> Option<u64> {
    spawn_with(name, entry, 0, 0, 0, 0, 0)
}

/// Crea una tarea que, al arrancar, ejecuta `entry` (en modo kernel) con el espacio
/// de direcciones `cr3` (0 = el del kernel). Para procesos de usuario, `entry` salta
/// a modo usuario en `user_rip` con la pila `user_rsp`.
pub fn spawn_user(
    name: &str,
    entry: fn(),
    cr3: u64,
    user_rip: u64,
    user_rsp: u64,
    arg0: u64,
    arg1: u64,
) -> Option<u64> {
    spawn_with(name, entry, cr3, user_rip, user_rsp, arg0, arg1)
}

fn spawn_with(
    name: &str,
    entry: fn(),
    cr3: u64,
    user_rip: u64,
    user_rsp: u64,
    user_arg0: u64,
    user_arg1: u64,
) -> Option<u64> {
    cpu::without_interrupts(|| {
        let s = sched();
        // Reserva FALLIBLE: si un programa de usuario agota la memoria, spawn debe fallar
        // con un error, no provocar un pánico del kernel.
        let mut stack: Vec<u8> = Vec::new();
        stack.try_reserve_exact(STACK_SIZE).ok()?;
        stack.resize(STACK_SIZE, 0);
        s.tasks.try_reserve(1).ok()?;
        stack[..8].copy_from_slice(&CANARY.to_le_bytes());

        // Fabricamos la pila como si la tarea ya hubiera sido interrumpida dentro
        // de switch_context: 6 registros a cero y, encima, la dirección de retorno.
        // top-72 hace que, tras los 6 pop + ret, rsp quede alineado a 16 (exigido
        // por la ABI antes de un `call`).
        let top = (stack.as_ptr() as usize + STACK_SIZE) & !15;
        let frame = (top - 72) as *mut u64;
        unsafe {
            for i in 0..6 {
                frame.add(i).write(0);
            }
            frame.add(6).write(thread_trampoline as *const () as usize as u64);
        }

        let id = s.next_id;
        s.next_id += 1;
        s.tasks.push(Box::new(Task {
            id,
            name: String::from(name),
            state: State::Ready,
            rsp: frame as usize,
            stack,
            entry: Some(entry),
            ticks: 0,
            idle: false,
            cr3,
            kstack_top: top as u64,
            user_rip,
            user_rsp,
            user_arg0,
            user_arg1,
            brk_start: crate::process::USER_HEAP_BASE,
            brk_cur: crate::process::USER_HEAP_BASE,
        }));
        Some(id)
    })
}

/// Elige la siguiente tarea lista (round-robin) y cambia a ella.
///
/// # Safety
/// Debe llamarse con las interrupciones DESACTIVADAS.
unsafe fn schedule() {
    let s = sched();
    let n = s.tasks.len();
    if n <= 1 {
        return;
    }
    let cur_pos = s.tasks.iter().position(|t| t.id == s.current).unwrap();

    // ¿Desbordó su pila la tarea que sale?
    {
        let t = &s.tasks[cur_pos];
        if !t.stack.is_empty() {
            let c = u64::from_le_bytes(t.stack[..8].try_into().unwrap());
            if c != CANARY {
                panic!("desbordamiento de pila en la tarea '{}' (id {})", t.name, t.id);
            }
        }
    }

    // 1) Primero, la siguiente tarea NORMAL que esté lista (round-robin).
    let mut next_pos = None;
    for off in 1..n {
        let p = (cur_pos + off) % n;
        if s.tasks[p].state == State::Ready && !s.tasks[p].idle {
            next_pos = Some(p);
            break;
        }
    }
    // 2) Si no hay ninguna: si la actual sigue en marcha, continúa; si se ha
    //    bloqueado (o terminado), toca la tarea ociosa.
    let np = match next_pos {
        Some(p) => p,
        None => {
            if s.tasks[cur_pos].state == State::Running {
                return;
            }
            match s.tasks.iter().position(|t| t.idle && t.state == State::Ready) {
                Some(p) => p,
                None => return,
            }
        }
    };

    let old: *mut Task = &mut *s.tasks[cur_pos];
    let new: *mut Task = &mut *s.tasks[np];
    if (*old).state == State::Running {
        (*old).state = State::Ready;
    }
    (*new).state = State::Running;
    s.current = (*new).id;

    // Cambio de espacio de direcciones (si hace falta) y de pila de kernel de la CPU.
    let new_cr3 = if (*new).cr3 != 0 { (*new).cr3 } else { s.kernel_cr3 };
    if new_cr3 != paging::read_cr3() & !0xfff {
        paging::write_cr3(new_cr3);
    }
    if (*new).kstack_top != 0 {
        gdt::set_kernel_stack((*new).kstack_top); // al llegar una IRQ desde ring 3
        syscall::set_kernel_stack((*new).kstack_top); // al hacer SYSCALL
    }

    switch_context(&raw mut (*old).rsp, (*new).rsp);

    // Aquí solo se vuelve cuando OTRA tarea nos reanude. Aprovechamos para liberar
    // las pilas de tareas terminadas (no podemos liberar la pila que estamos usando).
    reap();
}

/// Elimina las tareas terminadas (salvo la actual). Requiere interrupciones desactivadas.
fn reap() {
    let s = sched();
    let cur = s.current;
    let mut i = 0;
    while i < s.tasks.len() {
        if s.tasks[i].state == State::Dead && s.tasks[i].id != cur {
            let t = s.tasks.remove(i);
            if t.cr3 != 0 {
                // Libera páginas y tablas del proceso. Seguro: el CR3 activo es otro.
                unsafe { paging::destroy_address_space(t.cr3) };
            }
            // `t` se destruye aquí: se libera también su pila de kernel.
        } else {
            i += 1;
        }
    }
}

/// Cede la CPU voluntariamente.
#[allow(dead_code)]
pub fn yield_now() {
    cpu::without_interrupts(|| unsafe { schedule() });
}

/// Llamada desde la IRQ del temporizador en cada tick.
pub fn preempt() {
    let s = sched();
    if s.tasks.is_empty() {
        return; // el planificador aún no está iniciado
    }
    let cur = s.current;
    let now = crate::timer::ticks();
    for t in s.tasks.iter_mut() {
        if t.id == cur {
            t.ticks += 1;
        }
        // Despertar a las tareas dormidas cuyo plazo ya venció.
        if let State::Sleeping(wake) = t.state {
            if now >= wake {
                t.state = State::Ready;
            }
        }
    }
    unsafe { schedule() };
}

/// Bloquea la tarea actual hasta el tick `wake`. No gasta CPU mientras duerme.
pub fn sleep_until(wake: u64) {
    if sched().tasks.is_empty() {
        // Planificador sin iniciar: espera "a lo bruto".
        while crate::timer::ticks() < wake {
            unsafe { asm!("hlt", options(nomem, nostack)) };
        }
        return;
    }
    cpu::without_interrupts(|| {
        if crate::timer::ticks() >= wake {
            return;
        }
        let s = sched();
        let cur = s.current;
        if let Some(t) = s.tasks.iter_mut().find(|t| t.id == cur) {
            t.state = State::Sleeping(wake);
        }
        unsafe { schedule() };
    });
}

/// Bloquea la tarea actual hasta que haya entrada. La comprobación y el bloqueo se
/// hacen con interrupciones desactivadas: si no, una tecla podría llegar justo
/// entre las dos cosas y nadie despertaría a la tarea (un "lost wake-up").
pub fn wait_for_input() {
    cpu::without_interrupts(|| {
        if crate::input::has_data() || sched().tasks.is_empty() {
            return;
        }
        let s = sched();
        let cur = s.current;
        if let Some(t) = s.tasks.iter_mut().find(|t| t.id == cur) {
            t.state = State::WaitInput;
        }
        unsafe { schedule() };
    });
}

/// Despierta a las tareas que esperan entrada. La llama `input::push` desde una IRQ.
pub fn wake_input() {
    for t in sched().tasks.iter_mut() {
        if t.state == State::WaitInput {
            t.state = State::Ready;
        }
    }
}

/// Punto de entrada de toda tarea nueva (lo llama thread_trampoline).
#[no_mangle]
extern "C" fn thread_start() -> ! {
    let entry = cpu::without_interrupts(|| {
        reap();
        let s = sched();
        let cur = s.current;
        s.tasks.iter().find(|t| t.id == cur).and_then(|t| t.entry)
    });
    if let Some(f) = entry {
        f();
    }
    exit_current()
}

/// Termina la tarea actual con código 0.
pub fn exit_current() -> ! {
    exit_with(0)
}

/// Termina la tarea actual: la marca muerta, guarda su código de salida, despierta a
/// quien la espere y cambia a otra tarea. No vuelve.
pub fn exit_with(code: i64) -> ! {
    cpu::without_interrupts(|| {
        let s = sched();
        let cur = s.current;
        for t in s.tasks.iter_mut() {
            if t.id == cur {
                t.state = State::Dead;
            }
        }
        // Guardar el código hasta que alguien lo recoja con wait (solo los 64 últimos).
        if s.exit_codes.len() >= 64 {
            s.exit_codes.remove(0);
        }
        s.exit_codes.push((cur, code));
        for t in s.tasks.iter_mut() {
            if t.state == State::WaitExit(cur) {
                t.state = State::Ready;
            }
        }
        unsafe { schedule() }; // no vuelve: nadie reanuda una tarea muerta
    });
    loop {
        unsafe { asm!("hlt") };
    }
}

pub const ECHILD: i64 = -10;

/// Espera a que termine el proceso `pid` y devuelve su código de salida. Si ya había
/// terminado, devuelve el código guardado sin esperar. `ECHILD` si no existe (o ya se
/// recogió). La comprobación y el bloqueo van juntos con interrupciones desactivadas
/// para que el aviso de salida no se pierda.
pub fn wait_for_exit(pid: u64) -> i64 {
    loop {
        let r = cpu::without_interrupts(|| {
            let s = sched();
            if let Some(i) = s.exit_codes.iter().position(|(p, _)| *p == pid) {
                return Some(s.exit_codes.remove(i).1);
            }
            let alive = s.tasks.iter().any(|t| t.id == pid && t.state != State::Dead);
            if pid == s.current || !alive {
                return Some(ECHILD);
            }
            let cur = s.current;
            for t in s.tasks.iter_mut() {
                if t.id == cur {
                    t.state = State::WaitExit(pid);
                }
            }
            unsafe { schedule() };
            None // nos han despertado: volver a mirar la tabla
        });
        if let Some(code) = r {
            return code;
        }
    }
}

pub fn task_count() -> usize {
    cpu::without_interrupts(|| sched().tasks.len())
}

/// (base, fin actual) del heap de usuario de la tarea actual.
pub fn brk_info() -> (u64, u64) {
    cpu::without_interrupts(|| {
        let s = sched();
        let cur = s.current;
        s.tasks.iter().find(|t| t.id == cur).map(|t| (t.brk_start, t.brk_cur)).unwrap_or((0, 0))
    })
}

pub fn set_brk(new: u64) {
    cpu::without_interrupts(|| {
        let s = sched();
        let cur = s.current;
        if let Some(t) = s.tasks.iter_mut().find(|t| t.id == cur) {
            t.brk_cur = new;
        }
    })
}

pub struct TaskInfo {
    pub id: u64,
    pub name: String,
    pub state: &'static str,
    pub ticks: u64,
    pub user: bool,
}

pub fn list() -> Vec<TaskInfo> {
    cpu::without_interrupts(|| {
        sched()
            .tasks
            .iter()
            .map(|t| TaskInfo {
                id: t.id,
                name: t.name.clone(),
                state: match t.state {
                    State::Ready => "lista",
                    State::Running => "ejecutando",
                    State::Sleeping(_) => "durmiendo",
                    State::WaitInput => "esp.entrada",
                    State::WaitExit(_) => "esp.hijo",
                    State::Dead => "terminada",
                },
                ticks: t.ticks,
                user: t.cr3 != 0,
            })
            .collect()
    })
}

pub fn current_name() -> String {
    cpu::without_interrupts(|| {
        let s = sched();
        let cur = s.current;
        s.tasks.iter().find(|t| t.id == cur).map(|t| t.name.clone()).unwrap_or_default()
    })
}

pub fn current_id() -> u64 {
    sched().current
}

/// (rip, rsp, arg0, arg1) de usuario de la tarea actual.
pub fn current_user_entry() -> (u64, u64, u64, u64) {
    cpu::without_interrupts(|| {
        let s = sched();
        let cur = s.current;
        s.tasks
            .iter()
            .find(|t| t.id == cur)
            .map(|t| (t.user_rip, t.user_rsp, t.user_arg0, t.user_arg1))
            .unwrap_or((0, 0, 0, 0))
    })
}
