use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;

use crate::stem_separator_registry;

pub type StemMap = BTreeMap<String, Vec<f32>>;

pub struct StemExtractionTask {
    pub track_id: u32,
    pub input_samples: Arc<Vec<f32>>,
    pub sample_rate: f64,
    pub on_complete: Box<dyn FnOnce(StemMap) + Send>,
}

struct TaskQueue {
    tasks: Mutex<VecDeque<StemExtractionTask>>,
    ready: Condvar,
}

/// Offline stem separation on background workers. Separation is never an
/// audio-callback operation; the heuristic registry is the default provider.
pub struct StemExtractionPipeline {
    queue: Arc<TaskQueue>,
}

impl StemExtractionPipeline {
    pub fn new() -> Self {
        let workers = thread::available_parallelism()
            .map(|parallelism| parallelism.get() / 2)
            .unwrap_or(0)
            .max(1);
        let queue = Arc::new(TaskQueue {
            tasks: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
        });
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            thread::spawn(move || worker(queue));
        }
        Self { queue }
    }

    pub fn global() -> &'static Self {
        static PIPELINE: OnceLock<StemExtractionPipeline> = OnceLock::new();
        PIPELINE.get_or_init(Self::new)
    }

    /// Never runs the task on the calling (UI/control) thread.
    pub fn enqueue(&self, task: StemExtractionTask) {
        let mut tasks = self
            .queue
            .tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        tasks.push_back(task);
        drop(tasks);
        self.queue.ready.notify_one();
    }
}

impl Default for StemExtractionPipeline {
    fn default() -> Self {
        Self::new()
    }
}

fn worker(queue: Arc<TaskQueue>) {
    loop {
        let task = {
            let mut tasks = queue
                .tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                if let Some(task) = tasks.pop_front() {
                    break task;
                }
                tasks = queue
                    .ready
                    .wait(tasks)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        process(task);
    }
}

fn process(task: StemExtractionTask) {
    let samples = task.input_samples.as_slice();
    let n = samples.len();
    if n == 0 || n > u32::MAX as usize || !task.sample_rate.is_finite() || task.sample_rate <= 0.0
    {
        return;
    }
    let stereo = vec![samples.to_vec(), samples.to_vec()];
    let Some(separated) = stem_separator_registry::global().split(&stereo, task.sample_rate) else {
        return;
    };
    let copy_mono = |source: &[Vec<f32>]| {
        let mut result = vec![0.0f32; n];
        if let Some(left) = source.first() {
            for (target, sample) in result.iter_mut().zip(left.iter()) {
                *target = *sample;
            }
        }
        result
    };
    let mut stems = StemMap::new();
    stems.insert("vocals".to_string(), copy_mono(&separated.vocals));
    stems.insert("drums".to_string(), copy_mono(&separated.drums));
    stems.insert("bass".to_string(), copy_mono(&separated.bass));
    stems.insert("other".to_string(), copy_mono(&separated.other));
    (task.on_complete)(stems);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stem_splitter::split_stems;
    use std::sync::mpsc;
    use std::time::Duration;

    fn tone(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| (index as f32 * 0.11).sin())
            .collect()
    }

    #[test]
    fn pipeline_separates_on_background_workers_and_reports_all_stems() {
        let samples = Arc::new(tone(256));
        let (sender, receiver) = mpsc::channel();
        StemExtractionPipeline::global().enqueue(StemExtractionTask {
            track_id: 7,
            input_samples: Arc::clone(&samples),
            sample_rate: 44_100.0,
            on_complete: Box::new(move |stems| {
                sender.send(stems).unwrap();
            }),
        });
        let stems = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("pipeline completion within five seconds");
        for name in ["vocals", "drums", "bass", "other"] {
            let plane = stems.get(name).expect(name);
            assert_eq!(plane.len(), samples.len());
        }

        let stereo = vec![samples.as_ref().clone(), samples.as_ref().clone()];
        let direct = split_stems(&stereo, 44_100.0).unwrap();
        assert_eq!(stems["vocals"], direct.vocals[0]);
        assert_eq!(stems["drums"], direct.drums[0]);
        assert_eq!(stems["bass"], direct.bass[0]);
        assert_eq!(stems["other"], direct.other[0]);
    }

    #[test]
    fn invalid_tasks_complete_without_calling_back() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        for (samples, rate) in [(Vec::new(), 44_100.0), (tone(16), 0.0), (tone(16), f64::NAN)] {
            let flag = Arc::clone(&called);
            process(StemExtractionTask {
                track_id: 1,
                input_samples: Arc::new(samples),
                sample_rate: rate,
                on_complete: Box::new(move |_| {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }),
            });
        }
        assert!(!called.load(std::sync::atomic::Ordering::Relaxed));
    }
}
