use super::*;
use crate::loom_tests::bounded;

#[test]
fn loom_webhook_notifications_register_without_losing_or_doubling_wake() {
    bounded(1, || {
        let pending = Notifications::new();
        for _ in 0..3 {
            pending.wake();
        }
        let (sender, receiver) = mpsc::channel();
        pending.register(sender);
        assert!(matches!(receiver.recv().unwrap(), Input::GitHubWake));
        assert!(
            receiver.try_recv().is_err(),
            "pending wakes were not coalesced"
        );

        let notifications = Arc::new(Notifications::new());
        let receipt = Arc::clone(&notifications);
        let (sender, receiver) = mpsc::channel();
        let wake = thread::spawn(move || receipt.wake());
        notifications.register(sender);
        wake.join().unwrap();
        assert!(
            matches!(receiver.recv().unwrap(), Input::GitHubWake),
            "wake lost during registration"
        );
        assert!(receiver.try_recv().is_err(), "registration doubled wake");
    });
}
