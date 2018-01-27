use ethereum_types::{U256, H256, Address};
use vm;
use wasmi::{self, MemoryRef, RuntimeArgs, RuntimeValue, Error as InterpreterError};

pub struct RuntimeContext {
	pub address: Address,
	pub sender: Address,
	pub origin: Address,
	pub code_address: Address,
	pub value: U256,
}

struct Runtime<'a> {
	gas_counter: u64,
	gas_limit: u64,
	dynamic_top: u32,
	ext: &'a mut vm::Ext,
	context: RuntimeContext,
	memory: MemoryRef,
}

/// User trap in native code
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
	/// Storage read error
	StorageReadError,
	/// Storage update error
	StorageUpdateError,
	/// Memory access violation
	MemoryAccessViolation,
	/// Native code resulted in suicide
	Suicide,
	/// Suicide was requested but coudn't complete
	SuicideAbort,
	/// Invalid gas state inside interpreter
	InvalidGasState,
	/// Query of the balance resulted in an error
	BalanceQueryError,
	/// Failed allocation
	AllocationFailed,
	/// Gas limit reached
	GasLimit,
	/// Unknown runtime function
	Unknown,
	/// Passed string had invalid utf-8 encoding
	BadUtf8,
	/// Log event error
	Log,
	/// Other error in native code
	Other,
	/// Syscall signature mismatch
	InvalidSyscall,
	/// Panic with message
	Panic(String),
}

impl ::std::fmt::Display for Error {
	fn fmt(&self, f: &mut ::std::fmt::Formatter) -> Result<(), ::std::fmt::Error> {
		match *self {
			Error::StorageReadError => write!(f, "Storage read error"),
			Error::StorageUpdateError => write!(f, "Storage update error"),
			Error::MemoryAccessViolation => write!(f, "Memory access violation"),
			Error::SuicideAbort => write!(f, "Attempt to suicide resulted in an error"),
			Error::InvalidGasState => write!(f, "Invalid gas state"),
			Error::BalanceQueryError => write!(f, "Balance query resulted in an error"),
			Error::Suicide => write!(f, "Suicide result"),
			Error::Unknown => write!(f, "Unknown runtime function invoked"),
			Error::AllocationFailed => write!(f, "Memory allocation failed (OOM)"),
			Error::BadUtf8 => write!(f, "String encoding is bad utf-8 sequence"),
			Error::GasLimit => write!(f, "Invocation resulted in gas limit violated"),
			Error::Log => write!(f, "Error occured while logging an event"),
			Error::InvalidSyscall => write!(f, "Invalid syscall signature encountered at runtime"),
			Error::Other => write!(f, "Other unspecified error"),
			Error::Panic(ref msg) => write!(f, "Panic: {}", msg),
		}
	}
}

impl<'a> Runtime<'a> {
	/// New runtime for wasm contract with specified params
	pub fn with_params(
		ext: &mut vm::Ext,
		memory: MemoryRef,
		stack_space: u32,
		gas_limit: u64,
		context: RuntimeContext,
	) -> Runtime {
		Runtime {
			gas_counter: 0,
			gas_limit: gas_limit,
			dynamic_top: stack_space,
			memory: memory,
			ext: ext,
			context: context,
		}
	}

	fn h256_at(&self, ptr: u32) -> Result<H256, Error> {
		let mut buf = [0u8; 32];
		self.memory.get_into(ptr, &mut buf[..])
			.map_err(|e| Error::MemoryAccessViolation);

		Ok(H256::from(&buf[..]))
	}

	fn charge_gas(&mut self, amount: u64) -> bool {
		let prev = self.gas_counter;
		if prev + amount > self.gas_limit {
			// exceeds gas
			false
		} else {
			self.gas_counter = prev + amount;
			true
		}
	}

	/// Charge gas according to closure
	pub fn charge<F>(&mut self, f: F) -> Result<(), Error>
		where F: FnOnce(&vm::Schedule) -> u64
	{
		let amount = f(self.ext.schedule());
		if !self.charge_gas(amount as u64) {
			Err(Error::GasLimit)
		} else {
			Ok(())
		}
	}

	/// Read from the storage to wasm memory
	pub fn storage_read(&mut self, args: RuntimeArgs) -> Result<(), Error>
	{
		let key = self.h256_at(args.nth(0).map_err(|_| Error::InvalidSyscall)?)
			.map_err(|_| Error::MemoryAccessViolation)?;
		let val_ptr: u32 = args.nth(1).map_err(|_| Error::InvalidSyscall)?;

		let val = self.ext.storage_at(&key).map_err(|_| Error::StorageReadError)?;

		self.charge(|schedule| schedule.sload_gas as u64)?;

		self.memory.set(val_ptr as u32, &*val)
			.map_err(|_| Error::MemoryAccessViolation)?;

		Ok(())
	}
}