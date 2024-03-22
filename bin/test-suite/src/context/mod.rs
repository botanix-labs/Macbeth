pub struct Context {
    pub test_suite_id: uuid::Uuid,
    pub dry_run: bool,
}

impl Context {
    pub fn new(dry_run: bool) -> Self {
        Self { test_suite_id: uuid::Uuid::new_v4(), dry_run }
    }
}
