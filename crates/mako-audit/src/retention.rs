#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetentionMode {
    DryRun,
    Apply,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetailCompactionReport {
    pub retained_from_unix_milliseconds: u64,
    pub dry_run: bool,
    pub records_eligible: usize,
    pub records_removed: usize,
}
