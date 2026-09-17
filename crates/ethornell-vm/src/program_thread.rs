use crate::{NativeOpcode, SysApi, Value, Vm, VmResult, native_call::opcodes};
use std::sync::Arc;

impl Vm {
    /// Dispatch the target CThread module, child-thread, FIFO, and callback
    /// handlers as one shared scheduler subsystem.
    pub(super) fn dispatch_program_thread_opcode<A>(
        &mut self,
        api: &mut A,
        opcode: NativeOpcode,
        trace_events: bool,
    ) -> VmResult<Option<Value>>
    where
        A: SysApi,
    {
        let result = match opcode {
            opcodes::SYS_LOAD_PROGRAM_MODULE => {
                // sub_488C00 pops file then archive and appends the decoded BP
                // image to the current CThread's module chain.
                let file = self.pop_string_lossy()?;
                let archive = self.pop_string_lossy()?;
                let mut program = api
                    .load_program(&archive, &file)
                    .unwrap_or_else(|| super::empty_loaded_program(format!("{archive}:{file}")));
                self.assign_program_instance(&mut program);
                Value::Int(self.append_target_loaded_program(program) as i32)
            }
            opcodes::SYS_FREE_LAST_PROGRAM_MODULE => {
                // The exact handler does not pop the legacy descriptor slot.
                let (remaining, freed) = self.free_last_target_program(trace_events);
                if let Some(program) = freed {
                    api.free_program(Value::Program(Arc::new(program)));
                }
                Value::Int(remaining)
            }
            opcodes::SYS_LOAD_PROGRAM_THREAD => {
                // sub_488D00 native pop order is data bytes, code bytes,
                // operand slots, file, archive.
                let data_bytes = self.pop_value()?;
                let code_bytes = self.pop_value()?;
                let operand_slots = self.pop_value()?;
                let file = self.pop_string_lossy()?;
                let archive = self.pop_string_lossy()?;
                let parameters = [data_bytes, code_bytes, operand_slots];
                let mut program = api
                    .load_program_ex(&archive, &file, &parameters)
                    .unwrap_or_else(|| super::empty_loaded_program(format!("{archive}:{file}")));
                self.assign_program_instance(&mut program);
                let thread_id = self
                    .start_async_program_with_args(
                        Value::Program(Arc::new(program)),
                        Vec::new(),
                        trace_events,
                    )
                    .unwrap_or(0);
                Value::Int(thread_id)
            }
            opcodes::SYS_CURRENT_THREAD_ID => Value::Int(self.thread.thread_id()),
            opcodes::SYS_THREAD_EXISTS => {
                let thread_or_program = self.pop_value()?;
                Value::Int(self.async_program_is_active(thread_or_program))
            }
            opcodes::SYS_ENQUEUE_MESSAGE => self.sys80_48_enqueue_message(trace_events)?,
            opcodes::SYS_DEQUEUE_MESSAGE => self.sys80_49_dequeue_message()?,
            opcodes::SYS_ENQUEUE_MESSAGE_ARRAY => {
                self.sys80_4a_enqueue_message_array(trace_events)?
            }
            opcodes::SYS_DEQUEUE_MESSAGE_ARRAY => self.sys80_4b_dequeue_message_array()?,
            opcodes::SYS_INVOKE_THREAD_CALLBACK => {
                self.sys80_4c_invoke_thread_callback(trace_events)?
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }
}
