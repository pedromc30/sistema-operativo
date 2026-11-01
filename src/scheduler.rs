//! Planificador expropiativo (round-robin) de hilos de kernel.
//!
//! Cada tarea tiene su propia pila. Cambiar de tarea = guardar los registros
//! "callee-saved" en la pila actual, guardar `rsp`, cargar el `rsp` de la otra
//! tarea y restaurar sus registros. El temporizador llama a `preempt()` en cada
//! tick, así que ninguna tarea necesita ceder la CPU voluntariamente.

use crate::cpu;
use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::arch::{asm, global_asm};

const STACK_SIZE: usize = 32 * 1024;
/// Valor centinela en el fondo de cada pila: si cambia, la tarea la desbordó.
const CANARY: u64 = 0xDEAD_C0DE_5AFE_57AC;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ready,
    Running,
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
}

struct Scheduler {
    tasks: Vec<Box<Task>>, // Box: la dirección de cada Task no cambia al crecer el Vec
    current: u64,          // id de la tarea en ejecución
    next_id: u64,
}

static mut SCHED: Scheduler = Scheduler { tasks: Vec::new(), current: 0, next_id: 1 };

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
        }));
        s.current = 0;
    });
}

/// Crea una tarea nueva que ejecutará `entry`. Devuelve su id.
pub fn spawn(name: &str, entry: fn()) -> u64 {
    cpu::without_interrupts(|| {
        let s = sched();
        let mut stack = vec![0u8; STACK_SIZE];
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
        }));
        id
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

    let mut next_pos = None;
    for off in 1..n {
        let p = (cur_pos + off) % n;
        if s.tasks[p].state == State::Ready {
            next_pos = Some(p);
            break;
        }
    }
    let Some(np) = next_pos else { return }; // nadie más listo: seguimos nosotros

    let old: *mut Task = &mut *s.tasks[cur_pos];
    let new: *mut Task = &mut *s.tasks[np];
    if (*old).state == State::Running {
        (*old).state = State::Ready;
    }
    (*new).state = State::Running;
    s.current = (*new).id;

    switch_context(&raw mut (*old).rsp, (*new).rsp);

    // Aquí solo se vuelve cuando OTRA tarea nos reanude. Aprovechamos para liberar
    // las pilas de tareas terminadas (no podemos liberar la pila que estamos usando).
    reap();
}

/// Elimina las tareas terminadas (salvo la actual). Requiere interrupciones desactivadas.
fn reap() {
    let s = sched();
    let cur = s.current;
    s.tasks.retain(|t| t.state != State::Dead || t.id == cur);
}

/// Cede la CPU voluntariamente.
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
    if let Some(t) = s.tasks.iter_mut().find(|t| t.id == cur) {
        t.ticks += 1;
    }
    unsafe { schedule() };
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
    exit()
}

fn exit() -> ! {
    cpu::without_interrupts(|| {
        let s = sched();
        let cur = s.current;
        for t in s.tasks.iter_mut() {
            if t.id == cur {
                t.state = State::Dead;
            }
        }
        unsafe { schedule() }; // no vuelve: nadie reanuda una tarea muerta
    });
    loop {
        unsafe { asm!("hlt") };
    }
}

pub struct TaskInfo {
    pub id: u64,
    pub name: String,
    pub state: &'static str,
    pub ticks: u64,
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
                    State::Dead => "terminada",
                },
                ticks: t.ticks,
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
