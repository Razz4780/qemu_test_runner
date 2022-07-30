use std::{
    sync::{mpsc, Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

pub struct NoWorkersError;

type Task = Box<dyn FnOnce() + Send>;

struct Tasks {
    queue: Vec<Task>,
    shutdown: bool,
}

pub struct Pool {
    workers: Vec<JoinHandle<()>>,
    tasks: Arc<Mutex<Tasks>>,
    task_waiting: Arc<Condvar>,
}

impl Pool {
    pub fn new(size: usize) -> Result<Self, NoWorkersError> {
        if size == 0 {
            return Err(NoWorkersError);
        }

        let tasks = Arc::new(Mutex::new(Tasks {
            queue: Vec::new(),
            shutdown: false,
        }));
        let task_waiting = Arc::new(Condvar::new());

        let mut workers = Vec::new();
        workers.resize_with(size, || {
            let worker = Worker::new(tasks.clone(), task_waiting.clone());
            thread::spawn(move || worker.run())
        });

        Ok(Self {
            workers,
            tasks,
            task_waiting,
        })
    }

    pub fn exec<F, T>(&self, fun: F) -> T
    where
        F: 'static + Send + FnOnce() -> T,
        T: 'static + Send,
    {
        let (tx, rx) = mpsc::channel();
        let wrapped = move || {
            let res = fun();
            tx.send(res).unwrap();
        };
        self.tasks
            .lock()
            .expect("mutex poisoned")
            .queue
            .push(Box::new(wrapped));
        self.task_waiting.notify_one();
        rx.recv().expect("pool worker panicked")
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        if let Ok(mut handle) = self.tasks.lock() {
            handle.shutdown = true;
        }

        for worker in self.workers.drain(..) {
            worker.join().ok();
        }
    }
}

struct Worker {
    tasks: Arc<Mutex<Tasks>>,
    task_waiting: Arc<Condvar>,
}

impl Worker {
    fn new(tasks: Arc<Mutex<Tasks>>, task_waiting: Arc<Condvar>) -> Self {
        Self {
            tasks,
            task_waiting,
        }
    }

    fn get_task(&self) -> Option<Task> {
        let mut handle = self.tasks.lock().expect("mutex poisoned");

        while !handle.shutdown {
            if let Some(task) = handle.queue.pop() {
                return Some(task);
            }

            handle = self.task_waiting.wait(handle).expect("mutex poisoned");
        }

        None
    }

    fn run(self) {
        while let Some(task) = self.get_task() {
            task();
        }
    }
}
