use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use callisto_model::{CommandError, CommandOutput, CommandRunner};

#[derive(Default)]
pub struct ReplayCommandRunner {
    responses: BTreeMap<(String, Vec<String>), Result<CommandOutput, CommandError>>,
    calls: Mutex<Vec<(String, Vec<String>)>>,
}

impl ReplayCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on(mut self, program: &str, args: &[&str], out: CommandOutput) -> Self {
        let args_vec = args.iter().map(|s| s.to_string()).collect();
        self.responses
            .insert((program.to_string(), args_vec), Ok(out));
        self
    }

    pub fn on_error(mut self, program: &str, args: &[&str], err: CommandError) -> Self {
        let args_vec = args.iter().map(|s| s.to_string()).collect();
        self.responses
            .insert((program.to_string(), args_vec), Err(err));
        self
    }

    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for ReplayCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _cwd: &Path,
    ) -> Result<CommandOutput, CommandError> {
        let args_vec: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.calls
            .lock()
            .unwrap()
            .push((program.to_string(), args_vec.clone()));

        if let Some(res) = self.responses.get(&(program.to_string(), args_vec.clone())) {
            res.clone()
        } else {
            panic!(
                "ReplayCommandRunner: unexpected invocation `{}` with args {:?}",
                program, args_vec
            );
        }
    }
}
