//! One bounded system mutation; no paths, commands or user-history access in the helper.
use crate::{same_name, Scope, Variable};
mod native;
pub use crate::MAX_VALUE_BYTES;
pub use native::{change_system, dispatch_helper, require_unelevated, LocalSystemChannel};
pub const MAX_FRAME_BYTES: usize = 2 * MAX_VALUE_BYTES + 65536;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemRequest {
    pub name: String,
    pub expected: Option<Variable>,
    pub desired: Option<Variable>,
}
impl SystemRequest {
    pub fn validate(&self) -> Result<(), String> {
        if !crate::valid_name(&self.name) || (self.expected.is_none() && self.desired.is_none()) {
            return Err("Invalid system operation".into());
        }
        for state in [&self.expected, &self.desired].into_iter().flatten() {
            if state.scope != Scope::System
                || !same_name(&state.name, &self.name)
                || state.read_only()
                || state.bytes.len() > MAX_VALUE_BYTES
            {
                return Err("Unsupported system value, name, type or size".into());
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = b"EVM\x01".to_vec();
        bytes.extend((self.name.len() as u32).to_le_bytes());
        bytes.extend(self.name.as_bytes());
        for state in [&self.expected, &self.desired] {
            bytes.push(u8::from(state.is_some()));
            if let Some(value) = state {
                bytes.extend(value.registry_type.to_le_bytes());
                bytes.extend((value.bytes.len() as u32).to_le_bytes());
                bytes.extend(&value.bytes);
            }
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let fail = || "Invalid helper request".to_owned();
        if bytes.len() > MAX_FRAME_BYTES || bytes.get(..4) != Some(b"EVM\x01") {
            return Err(fail());
        }
        struct Cursor<'a>(&'a [u8]);
        impl<'a> Cursor<'a> {
            fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
                if n > self.0.len() {
                    return Err("Truncated helper request".into());
                }
                let (head, tail) = self.0.split_at(n);
                self.0 = tail;
                Ok(head)
            }
            fn number(&mut self) -> Result<u32, String> {
                Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
            }
        }
        let mut cursor = Cursor(&bytes[4..]);
        let len = cursor.number()? as usize;
        if len > 65532 {
            return Err(fail());
        }
        let name = std::str::from_utf8(cursor.take(len)?)
            .map_err(|_| fail())?
            .to_owned();
        let mut state = || -> Result<Option<Variable>, String> {
            match cursor.take(1)?[0] {
                0 => Ok(None),
                1 => {
                    let registry_type = cursor.number()?;
                    let len = cursor.number()? as usize;
                    if len > MAX_VALUE_BYTES {
                        return Err(fail());
                    }
                    Ok(Some(Variable {
                        scope: Scope::System,
                        name: name.clone(),
                        registry_type,
                        bytes: cursor.take(len)?.to_vec(),
                    }))
                }
                _ => Err(fail()),
            }
        };
        let expected = state()?;
        let desired = state()?;
        if !cursor.0.is_empty() {
            return Err(fail());
        }
        let result = Self {
            name,
            expected,
            desired,
        };
        result.validate()?;
        Ok(result)
    }
}

/// Operating-system boundary for one variable, with no request-supplied key path.
/// The real helper supplies only its private, fixed HKLM environment adapter.
pub trait SystemEnvironment {
    fn read(&self, name: &str) -> Result<Option<Variable>, String>;
    fn replace(&self, name: &str, value: Option<&Variable>) -> Result<(), String>;
}
impl SystemRequest {
    /// Runs inside the helper after authorization/consent. Comparison is best effort,
    /// not atomic compare-and-swap against noncooperating external writers.
    pub fn apply(
        &self,
        environment: &dyn SystemEnvironment,
        caller_alive: impl Fn() -> bool,
    ) -> crate::WriteOutcome {
        use crate::WriteOutcome;
        if let Err(error) = self.validate() {
            return WriteOutcome::Rejected(error);
        }
        if !caller_alive() {
            return WriteOutcome::Rejected("Parent lifetime ended before mutation".into());
        }
        let current = match environment.read(&self.name) {
            Ok(current) => current,
            Err(error) => return WriteOutcome::Rejected(error),
        };
        if current != self.expected {
            return WriteOutcome::Conflict;
        }
        if !caller_alive() {
            return WriteOutcome::Rejected("Parent lifetime ended before mutation".into());
        }
        let write = environment.replace(&self.name, self.desired.as_ref());
        let readback = environment.read(&self.name);
        WriteOutcome::Attempted(match (write, readback) {
            (Ok(()), Ok(observed)) if observed == self.desired => Ok(()),
            _ => Err("System write failed or readback differed".into()),
        })
    }
}
