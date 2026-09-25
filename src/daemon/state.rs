use serde::{Deserialize, Serialize};
use session_activity::SessionKey;

use crate::{process::ProcessId, registry::Session};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIdentity {
    key: SessionKey,
    incarnation: String,
    process: Option<ProcessId>,
    started_at: i64,
}

impl From<&Session> for SessionIdentity {
    fn from(session: &Session) -> Self {
        Self {
            key: session.activity_key(),
            incarnation: session.incarnation.clone(),
            process: session.terminal.agent,
            started_at: session.started_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewContext {
    pub generation: u64,
    pub session: Option<SessionIdentity>,
}

impl ViewContext {
    pub fn accepts(&self, generation: u64, action: &Self) -> bool {
        self.generation == generation && self == action
    }

    pub fn owns(&self, session: &Session) -> bool {
        self.session.as_ref() == Some(&SessionIdentity::from(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_cannot_cross_views_or_resumed_sessions() {
        let session: Session = serde_json::from_value(serde_json::json!({
            "session_id":"s", "incarnation":"first", "cwd":"/tmp", "started_at":1, "last_active_at":1
        }))
        .unwrap();
        let context = ViewContext { generation: 4, session: Some((&session).into()) };
        assert!(context.accepts(4, &context));
        assert!(!context.accepts(5, &context));
        assert!(context.owns(&session));
        let resumed = Session { incarnation: "second".into(), ..session };
        assert!(!context.owns(&resumed));
    }
}
