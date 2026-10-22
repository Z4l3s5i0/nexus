use std::sync::{Mutex, mpsc};
use std::sync::atomic::{AtomicU8, Ordering};
use once_cell::sync::Lazy;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    None = 0,
    Exp = 1,
    Info = 2,
    Debug = 3,
}

static LOG_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info as u8);

pub static LOGS: Lazy<Mutex<VecDeque<String>>> = Lazy::new(|| Mutex::new(VecDeque::with_capacity(1000)));

pub enum LogType {
    Exp,
    Info,
    Debug,
    Warn,
    Error,
}

pub enum LogEvent {
    Log(LogType, String, String),
    Flush(mpsc::Sender<()>),
}

pub static LOG_CHANNEL: Lazy<mpsc::SyncSender<LogEvent>> = Lazy::new(|| {
    let (tx, rx) = mpsc::sync_channel::<LogEvent>(2048);
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            match event {
                LogEvent::Log(log_type, timestamp, message) => {
                    let (tag, is_error) = match log_type {
                        LogType::Exp => ("EXP  ", false),
                        LogType::Info => ("INFO ", false),
                        LogType::Debug => ("DEBUG", false),
                        LogType::Warn => ("WARN ", false),
                        LogType::Error => ("ERROR", true),
                    };

                    if is_error {
                        eprintln!("{} {} {}", timestamp, tag, message);
                    } else {
                        println!("{} {} {}", timestamp, tag, message);
                    }

                    let log_entry = format!("{} [{}] {}", timestamp, tag.trim(), message);
                    let mut logs = LOGS.lock().unwrap();
                    if logs.len() >= 1000 {
                        logs.pop_front();
                    }
                    logs.push_back(log_entry);
                }
                LogEvent::Flush(tx) => {
                    let _ = tx.send(());
                }
            }
        }
    });
    tx
});

pub fn flush() {
    let (tx, rx) = mpsc::channel();
    if let Ok(_) = LOG_CHANNEL.send(LogEvent::Flush(tx)) {
        let _ = rx.recv();
    }
}

pub fn set_log_level(level: LogLevel) {
    LOG_LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn get_log_level() -> LogLevel {
    match LOG_LEVEL.load(Ordering::Relaxed) {
        0 => LogLevel::None,
        1 => LogLevel::Exp,
        2 => LogLevel::Info,
        3 => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

pub fn add_log(log: String) {
    let now = chrono::Local::now();
    let timestamp = now.format("%b %d %H:%M:%S%.3f").to_string();
    let _ = LOG_CHANNEL.try_send(LogEvent::Log(LogType::Info, timestamp, log));
}

pub use chrono;

#[macro_export]
macro_rules! exp {
    ($($arg:tt)*) => {
        {
            if $crate::logging::get_log_level() >= $crate::logging::LogLevel::Exp {
                let now = $crate::logging::chrono::Local::now();
                let timestamp = now.format("%b %d %H:%M:%S%.3f").to_string();
                let message = format!($($arg)*);
                let _ = $crate::logging::LOG_CHANNEL.try_send($crate::logging::LogEvent::Log(
                    $crate::logging::LogType::Exp,
                    timestamp,
                    message,
                ));
            }
        }
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        {
            if $crate::logging::get_log_level() >= $crate::logging::LogLevel::Info {
                let now = $crate::logging::chrono::Local::now();
                let timestamp = now.format("%b %d %H:%M:%S%.3f").to_string();
                let message = format!($($arg)*);
                let _ = $crate::logging::LOG_CHANNEL.try_send($crate::logging::LogEvent::Log(
                    $crate::logging::LogType::Info,
                    timestamp,
                    message,
                ));
            }
        }
    };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        {
            if $crate::logging::get_log_level() >= $crate::logging::LogLevel::Debug {
                let now = $crate::logging::chrono::Local::now();
                let timestamp = now.format("%b %d %H:%M:%S%.3f").to_string();
                let message = format!($($arg)*);
                let _ = $crate::logging::LOG_CHANNEL.try_send($crate::logging::LogEvent::Log(
                    $crate::logging::LogType::Debug,
                    timestamp,
                    message,
                ));
            }
        }
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        {
            let now = $crate::logging::chrono::Local::now();
            let timestamp = now.format("%b %d %H:%M:%S%.3f").to_string();
            let message = format!($($arg)*);
            let _ = $crate::logging::LOG_CHANNEL.try_send($crate::logging::LogEvent::Log(
                $crate::logging::LogType::Warn,
                timestamp,
                message,
            ));
        }
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        {
            let now = $crate::logging::chrono::Local::now();
            let timestamp = now.format("%b %d %H:%M:%S%.3f").to_string();
            let message = format!($($arg)*);
            let _ = $crate::logging::LOG_CHANNEL.try_send($crate::logging::LogEvent::Log(
                $crate::logging::LogType::Error,
                timestamp,
                message,
            ));
        }
    };
}

