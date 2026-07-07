//! Shared utilities used by multiple modules.

/// Lock a plain `Mutex<T>`, recovering the guard on poison.
///
/// State inside this codebase is plain data — a poisoned lock should not
/// kill the loop. The guard is recovered so the caller can proceed.
pub fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Lock an `Arc<Mutex<T>>`, recovering the guard on poison.
///
/// Same semantics as [`lock`] but for `Arc<Mutex<T>>` which is the common
/// shape for shared state in this project.
pub fn lock_arc<T>(m: &std::sync::Arc<std::sync::Mutex<T>>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn lock_returns_guard() {
        let m = Mutex::new(42);
        let g = lock(&m);
        assert_eq!(*g, 42);
    }

    #[test]
    fn lock_arc_returns_guard() {
        let m = Arc::new(Mutex::new("hello"));
        let g = lock_arc(&m);
        assert_eq!(*g, "hello");
    }

    #[test]
    fn lock_recovers_on_poison() {
        // Spawn a thread that panics while holding the lock, poisoning it.
        let m = Arc::new(Mutex::new(0));
        let m2 = Arc::clone(&m);
        let handle = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison!");
        });
        // Wait for the thread to finish (and poison the lock).
        let _ = handle.join();

        // lock() should still succeed by recovering the guard.
        let mut g = lock(&m);
        *g = 99;
        assert_eq!(*g, 99);
    }

    #[test]
    fn lock_arc_recovers_on_poison() {
        let m = Arc::new(Mutex::new(0));
        let m2 = Arc::clone(&m);
        let handle = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison!");
        });
        let _ = handle.join();

        let mut g = lock_arc(&m);
        *g = 42;
        assert_eq!(*g, 42);
    }

    #[test]
    fn lock_works_concurrently() {
        let m = Arc::new(Mutex::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let m = Arc::clone(&m);
                std::thread::spawn(move || {
                    let mut g = lock_arc(&m);
                    *g += 1;
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*lock(&m), 8);
    }
}
