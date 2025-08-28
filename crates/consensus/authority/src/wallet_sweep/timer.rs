use std::{pin::Pin, time::Duration};
use tokio::time::{sleep, Sleep};

#[derive(Debug, Default)]
pub enum SigningHandlerTimer {
    /// Never run
    #[default]
    None,
    /// Run after a delay
    Timer(Pin<Box<Sleep>>),
}

impl SigningHandlerTimer {
    pub fn after(d: Duration) -> Self {
        SigningHandlerTimer::Timer(Box::pin(sleep(d)))
    }

    pub fn immediately() -> Self {
        SigningHandlerTimer::Timer(Box::pin(sleep(Duration::from_secs(0))))
    }

    pub fn pause() -> Self {
        SigningHandlerTimer::None
    }

    pub async fn wait(&mut self) {
        match self {
            SigningHandlerTimer::None => futures::future::pending::<()>().await,
            SigningHandlerTimer::Timer(ref mut timer) => Pin::as_mut(timer).await,
        }

        // Disable schedule after it's triggered
        *self = Self::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{advance, Instant};

    /// Helper that executes `schedule.wait()` in a task and returns a
    /// `tokio::sync::oneshot::Receiver<()>` that is fulfilled once the wait
    /// future completes.
    async fn spawn_wait(mut schedule: SigningHandlerTimer) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            schedule.wait().await;
            let _ = tx.send(());
        });
        rx
    }

    #[tokio::test(start_paused = true)]
    async fn schedule_immediately_fires() {
        let mut schedule = SigningHandlerTimer::immediately();

        // Should finish without advancing the clock.
        let before = Instant::now();
        schedule.wait().await;
        let elapsed = Instant::now() - before;

        assert!(elapsed.is_zero(), "immediately() must resolve instantly");
    }

    #[tokio::test(start_paused = true)]
    async fn schedule_after_fires_after_delay() {
        let delay = Duration::from_secs(5);
        let schedule = SigningHandlerTimer::after(delay);

        // Spawn the wait future and observe when it completes.
        let mut rx = spawn_wait(schedule).await;

        // It must *not* be ready before `delay`.
        advance(delay - Duration::from_millis(1)).await;
        assert!(rx.try_recv().is_err(), "timer fired too early");

        // And it must be ready once `delay` has fully elapsed.
        advance(Duration::from_millis(1)).await;
        assert!(rx.await.is_ok(), "timer did not fire after the delay");
    }

    #[tokio::test(start_paused = true)]
    async fn schedule_works_inside_tokio_select() {
        use advance;
        use sleep;

        let mut schedule = SigningHandlerTimer::after(Duration::from_secs(3));

        advance(Duration::from_secs(1)).await; // make 1-s timer ready

        let mut short = sleep(Duration::from_secs(0)); // ready immediately
        tokio::pin!(short);

        let mut wait_fut = schedule.wait(); // !Unpin
        tokio::pin!(wait_fut); // <-- pin it

        tokio::select! {
            _ = &mut short    => {},                    // must win
            _ = &mut wait_fut => panic!("schedule fired too early"),
        }
    }

    /// After `wait()` resolves once, further `wait()`s must never finish.
    #[tokio::test(start_paused = true)]
    async fn schedule_fires_once_then_disarms() {
        let delay = Duration::from_secs(2);
        let mut schedule = SigningHandlerTimer::after(delay);

        // Trigger the first time.
        advance(delay).await;
        schedule.wait().await;

        // `schedule` is now `None`. Move it into a background task and ensure it
        // never resolves. To do so we need to take ownership of the schedule.
        let schedule = std::mem::take(&mut schedule);
        let mut rx = spawn_wait(schedule).await;

        // Advance virtual time well beyond any reasonable timeout.
        advance(Duration::from_secs(10)).await;

        // The channel must still be empty, meaning the second wait didn't fire.
        assert!(
            rx.try_recv().is_err(),
            "schedule triggered a second time, but it should be disarmed"
        );
    }
}
