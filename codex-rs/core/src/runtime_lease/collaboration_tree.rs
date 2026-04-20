use std::fmt;

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CollaborationTreeId(String);

#[allow(dead_code)]
impl CollaborationTreeId {
    pub(crate) fn root_for_session(session_id: &str) -> Self {
        Self(format!("session:{session_id}"))
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Display for CollaborationTreeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
