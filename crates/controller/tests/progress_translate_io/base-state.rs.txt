#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AllowanceState {
    pub delivery: Delivery,
    #[serde(default)]
    pub maintenance: bool,
    due_ms: Option<u64>,
    progress: Option<String>,
    progress_ms: u64,
    quota_ms: Option<u64>,
    #[serde(default)]
    last_quota_ms: Option<u64>,
    #[serde(default)]
    last_quota_model: Option<String>,
    /// #759: since this step's first model ending with no model turn served
    /// since. A progress checkpoint clears it.
    #[serde(default)]
    unserved_since_ms: Option<u64>,
    /// #759: the operations note for this run of endings is posted.
    #[serde(default)]
    ladder_out_noticed: bool,
    stopping: bool,
    pub incident: Option<String>,
    #[serde(default)]
    healthy: bool,
    #[serde(default)]
    found: Option<String>,
    #[serde(default)]
    failure: Option<String>,
    #[serde(default)]
    read_failures: u8,
    #[serde(default)]
    read_incidents: u8,
    /// When the last failed read was counted.
    #[serde(default)]
    counted_failure_ms: Option<u64>,
    #[serde(default)]
    external_wait: Option<tl3_store::ExternalWait>,
    #[serde(default)]
    wait_ended: bool,
}

impl AllowanceState {
    pub(crate) fn new(delivery: Delivery, now: u64, delay: u64) -> Self {
        Self {
            delivery,
            maintenance: false,
            due_ms: Some(now.saturating_add(delay)),
            progress: None,
            progress_ms: now,
            quota_ms: None,
            last_quota_ms: None,
            last_quota_model: None,
            unserved_since_ms: None,
            ladder_out_noticed: false,
            stopping: false,
            incident: None,
            healthy: false,
            found: None,
            failure: None,
            read_failures: 0,
            read_incidents: 0,
            counted_failure_ms: None,
            external_wait: None,
            wait_ended: false,
        }
    }
}

