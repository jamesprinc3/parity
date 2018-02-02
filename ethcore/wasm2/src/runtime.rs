use ethereum_types::{U256, H256, Address};
use vm::{self, CallType};
use wasmi::{self, MemoryRef, RuntimeArgs, RuntimeValue, Error as InterpreterError};
use super::panic_payload;

pub struct RuntimeContext {
	pub address: Address,
	pub sender: Address,
	pub origin: Address,
	pub code_address: Address,
	pub value: U256,
}

pub struct Runtime<'a> {
	gas_counter: u64,
	gas_limit: u64,
	ext: &'a mut vm::Ext,
	context: RuntimeContext,
	memory: MemoryRef,
	args: Vec<u8>,
	result: Vec<u8>,
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

impl wasmi::HostError for Error { }

impl From<InterpreterError> for Error {
	fn from(interpreter_err: InterpreterError) -> Self {
		match interpreter_err {
			InterpreterError::Value(_) => Error::InvalidSyscall,
			InterpreterError::Memory(_) => Error::MemoryAccessViolation,
			_ => Error::Other,
		}
	}
}

impl ::std::fmt::Display for Error {
	fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::result::Result<(), ::std::fmt::Error> {
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

type Result<T> = ::std::result::Result<T, Error>;

impl<'a> Runtime<'a> {
	/// New runtime for wasm contract with specified params
	pub fn with_params(
		ext: &mut vm::Ext,
		memory: MemoryRef,
		gas_limit: u64,
		args: Vec<u8>,
		context: RuntimeContext,
	) -> Runtime {
		Runtime {
			gas_counter: 0,
			gas_limit: gas_limit,
			memory: memory,
			ext: ext,
			context: context,
			args: args,
			result: Vec::new(),
		}
	}

	fn h256_at(&self, ptr: u32) -> Result<H256> {
		let mut buf = [0u8; 32];
		self.memory.get_into(ptr, &mut buf[..])?;

		Ok(H256::from(&buf[..]))
	}

	fn address_at(&self, ptr: u32) -> Result<Address> {
		let mut buf = [0u8; 20];
		self.memory.get_into(ptr, &mut buf[..])?;

		Ok(Address::from(&buf[..]))
	}

	fn u256_at(&self, ptr: u32) -> Result<U256> {
		let mut buf = [0u8; 32];
		self.memory.get_into(ptr, &mut buf[..])?;

		Ok(U256::from_big_endian(&buf[..]))
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
	pub fn charge<F>(&mut self, f: F) -> Result<()>
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
	pub fn storage_read(&mut self, args: RuntimeArgs) -> Result<()>
	{
		let key = self.h256_at(args.nth(0)?)?;
		let val_ptr: u32 = args.nth(1)?;

		let val = self.ext.storage_at(&key).map_err(|_| Error::StorageReadError)?;

		self.charge(|schedule| schedule.sload_gas as u64)?;

		self.memory.set(val_ptr as u32, &*val)?;

		Ok(())
	}

	/// Read from the storage to wasm memory
	pub fn storage_write(&mut self, args: RuntimeArgs) -> Result<()>
	{
		let key = self.h256_at(args.nth(0)?)?;
		let val_ptr: u32 = args.nth(1)?;

		self.charge(|schedule| schedule.sstore_set_gas as u64)?;

		let val = self.h256_at(val_ptr)?;
		self.ext.set_storage(key, val).map_err(|_| Error::StorageUpdateError)?;

		Ok(())
	}

	pub fn schedule(&self) -> &vm::Schedule {
		self.ext.schedule()
	}

	pub fn ret(&mut self, args: RuntimeArgs) -> Result<()> {
		let ptr: u32 = args.nth(0)?;
		let len: u32 = args.nth(1)?;

		trace!(target: "wasm", "Contract ret: {} bytes @ {}", len, ptr);

		self.result = self.memory.get(ptr, len as usize)?;

		Ok(())
	}

	pub fn result(&self) -> &[u8] {
		&self.result
	}

	pub fn dissolve(self) -> Vec<u8> {
		self.result
	}

	/// Query current gas left for execution
	pub fn gas_left(&self) -> Result<u64> {
		if self.gas_counter > self.gas_limit { return Err(Error::InvalidGasState); }
		Ok(self.gas_limit - self.gas_counter)
	}

	/// Report gas cost with the params passed in wasm stack
	fn gas(&mut self, args: RuntimeArgs) -> Result<()> {
		let amount: u32 = args.nth(0)?;
		if self.charge_gas(amount as u64) {
			Ok(())
		} else {
			Err(Error::GasLimit.into())
		}
	}

	fn input_legnth(&mut self, args: RuntimeArgs) -> RuntimeValue {
		RuntimeValue::I32(self.args.len() as i32)
	}

	fn fetch_input(&mut self, args: RuntimeArgs) -> Result<()> {
		let ptr: u32 = args.nth(0)?;
		self.memory.set(ptr, &self.args[..])?;
		Ok(())
	}

	fn panic(&mut self, args: RuntimeArgs) -> Result<()>
	{
		let payload_ptr: u32 = args.nth(0)?;
		let payload_len: u32 = args.nth(1)?;

		let raw_payload = self.memory.get(payload_ptr, payload_len as usize)?;
		let payload = panic_payload::decode(&raw_payload);
		let msg = format!(
			"{msg}, {file}:{line}:{col}",
			msg = payload
				.msg
				.as_ref()
				.map(String::as_ref)
				.unwrap_or("<msg was stripped>"),
			file = payload
				.file
				.as_ref()
				.map(String::as_ref)
				.unwrap_or("<unknown>"),
			line = payload.line.unwrap_or(0),
			col = payload.col.unwrap_or(0)
		);
		trace!(target: "wasm", "Contract custom panic message: {}", msg);

		Err(Error::Panic(msg).into())
	}

	fn do_call(
		&mut self,
		use_val: bool,
		call_type: CallType,
		args: RuntimeArgs,
	)
		-> Result<RuntimeValue>
	{
		let gas: u64 = args.nth(0)?;
		let address = self.address_at(args.nth(1)?)?;

		let vofs = if use_val { 1 } else { 0 };
		let val = if use_val { Some(self.u256_at(args.nth(2)?)?) } else { None };
		let input_ptr: u32 = args.nth(2 + vofs)?;
		let input_len: u32 = args.nth(3 + vofs)?;
		let result_ptr: u32 = args.nth(4 + vofs)?;
		let result_alloc_len: u32 = args.nth(5 + vofs)?;

		trace!(target: "wasm", "runtime: CALL({:?})", call_type);
		trace!(target: "wasm", "    result_len: {:?}", result_alloc_len);
		trace!(target: "wasm", "    result_ptr: {:?}", result_ptr);
		trace!(target: "wasm", "     input_len: {:?}", input_len);
		trace!(target: "wasm", "     input_ptr: {:?}", input_ptr);
		trace!(target: "wasm", "           val: {:?}", val);
		trace!(target: "wasm", "       address: {:?}", address);
		trace!(target: "wasm", "           gas: {:?}", gas);

		if let Some(ref val) = val {
			let address_balance = self.ext.balance(&self.context.address)
				.map_err(|_| Error::BalanceQueryError)?;

			if &address_balance < val {
				trace!(target: "wasm", "runtime: call failed due to balance check");
				return Ok((-1i32).into());
			}
		}

		self.charge(|schedule| schedule.call_gas as u64)?;

		let mut result = Vec::with_capacity(result_alloc_len as usize);
		result.resize(result_alloc_len as usize, 0);

		// todo: optimize to use memory views once it's in
		let payload = self.memory.get(input_ptr, input_len as usize)?;

		self.charge(|_| gas.into())?;

		let call_result = self.ext.call(
			&gas.into(),
			match call_type { CallType::DelegateCall => &self.context.sender, _ => &self.context.address },
			match call_type { CallType::Call | CallType::StaticCall => &address, _ => &self.context.address },
			val,
			&payload,
			&address,
			&mut result[..],
			call_type,
		);

		match call_result {
			vm::MessageCallResult::Success(gas_left, _) => {
				// cannot overflow, before making call gas_counter was incremented with gas, and gas_left < gas
				self.gas_counter = self.gas_counter - gas_left.low_u64();

				self.memory.set(result_ptr, &result)?;
				Ok(0i32.into())
			},
			vm::MessageCallResult::Reverted(gas_left, _) => {
				// cannot overflow, before making call gas_counter was incremented with gas, and gas_left < gas
				self.gas_counter = self.gas_counter - gas_left.low_u64();

				self.memory.set(result_ptr, &result)?;
				Ok((-1i32).into())
			},
			vm::MessageCallResult::Failed  => {
				Ok((-1i32).into())
			}
		}
	}

	fn ccall(&mut self, args: RuntimeArgs) -> Result<RuntimeValue> {
		self.do_call(true, CallType::Call, args)
	}

	fn dcall(&mut self, args: RuntimeArgs) -> Result<RuntimeValue> {
		self.do_call(false, CallType::DelegateCall, args)
	}

	fn scall(&mut self, args: RuntimeArgs) -> Result<RuntimeValue> {
		self.do_call(false, CallType::StaticCall, args)
	}

	fn return_address_ptr(&mut self, ptr: u32, val: Address) -> Result<()>
	{
		self.charge(|schedule| schedule.wasm.static_address as u64)?;
		self.memory.set(ptr, &*val)?;
		Ok(())
	}

	fn return_u256_ptr(&mut self, ptr: u32, val: U256) -> Result<()> {
		let value: H256 = val.into();
		self.charge(|schedule| schedule.wasm.static_u256 as u64)?;
		self.memory.set(ptr, &*value)?;
		Ok(())
	}

	pub fn value(&mut self, args: RuntimeArgs) -> Result<()> {
		let val = self.context.value;
		self.return_u256_ptr(args.nth(0)?, val)
	}

	pub fn create(&mut self, args: RuntimeArgs) -> Result<RuntimeValue>
	{
		//
		// method signature:
		//   fn create(endowment: *const u8, code_ptr: *const u8, code_len: u32, result_ptr: *mut u8) -> i32;
		//
		trace!(target: "wasm", "runtime: CREATE");
		let endowment = self.u256_at(args.nth(0)?)?;
		trace!(target: "wasm", "       val: {:?}", endowment);
		let code_ptr: u32 = args.nth(1)?;
		trace!(target: "wasm", "  code_ptr: {:?}", code_ptr);
		let code_len: u32 = args.nth(2)?;
		trace!(target: "wasm", "  code_len: {:?}", code_len);
		let result_ptr: u32 = args.nth(3)?;
		trace!(target: "wasm", "result_ptr: {:?}", result_ptr);

		let code = self.memory.get(code_ptr, code_len as usize)?;

		self.charge(|schedule| schedule.create_gas as u64)?;
		self.charge(|schedule| schedule.create_data_gas as u64 * code.len() as u64)?;

		let gas_left: U256 = self.gas_left()?.into();

		match self.ext.create(&gas_left, &endowment, &code, vm::CreateContractAddress::FromSenderAndCodeHash) {
			vm::ContractCreateResult::Created(address, gas_left) => {
				self.memory.set(result_ptr, &*address)?;
				self.gas_counter = self.gas_limit - gas_left.low_u64();
				trace!(target: "wasm", "runtime: create contract success (@{:?})", address);
				Ok(0i32.into())
			},
			vm::ContractCreateResult::Failed => {
				trace!(target: "wasm", "runtime: create contract fail");
				Ok((-1i32).into())
			},
			vm::ContractCreateResult::Reverted(gas_left, _) => {
				trace!(target: "wasm", "runtime: create contract reverted");
				self.gas_counter = self.gas_limit - gas_left.low_u64();
				Ok((-1i32).into())
			},
		}
	}

	fn debug(&mut self, args: RuntimeArgs) -> Result<()>
	{
		let msg_ptr: u32 = args.nth(0)?;
		let msg_len: u32 = args.nth(1)?;

		let msg = String::from_utf8(self.memory.get(msg_ptr, msg_len as usize)?)
			.map_err(|_| Error::BadUtf8)?;

		trace!(target: "wasm", "Contract debug message: {}", msg);

		Ok(())
	}

	/// Pass suicide to state runtime
	pub fn suicide(&mut self, args: RuntimeArgs) -> Result<()>
	{
		let refund_address = self.address_at(args.nth(0)?)?;

		if self.ext.exists(&refund_address).map_err(|_| Error::SuicideAbort)? {
			trace!(target: "wasm", "Suicide: refund to existing address {}", refund_address);
			self.charge(|schedule| schedule.suicide_gas as u64)?;
		} else {
			trace!(target: "wasm", "Suicide: refund to new address {}", refund_address);
			self.charge(|schedule| schedule.suicide_to_new_account_cost as u64)?;
		}

		self.ext.suicide(&refund_address).map_err(|_| Error::SuicideAbort)?;

		// We send trap to interpreter so it should abort further execution
		Err(Error::Suicide.into())
	}

	pub fn blockhash(&mut self, args: RuntimeArgs) -> Result<()> {
		self.charge(|schedule| schedule.blockhash_gas as u64)?;
		let hash = self.ext.blockhash(&U256::from(args.nth::<u64>(0)?));
		self.memory.set(args.nth(1)?, &*hash)?;

		Ok(())
	}

	pub fn blocknumber(&mut self) -> Result<RuntimeValue> {
		Ok(RuntimeValue::from(self.ext.env_info().number))
	}

	pub fn coinbase(&mut self, args: RuntimeArgs) -> Result<()> {
		let coinbase = self.ext.env_info().author;
		self.return_address_ptr(args.nth(0)?, coinbase)
	}

	pub fn difficulty(&mut self, args: RuntimeArgs) -> Result<()> {
		let difficulty = self.ext.env_info().difficulty;
		self.return_u256_ptr(args.nth(0)?, difficulty)
	}

	pub fn gaslimit(&mut self, args: RuntimeArgs) -> Result<()> {
		let gas_limit = self.ext.env_info().gas_limit;
		self.return_u256_ptr(args.nth(0)?, gas_limit)
	}

	pub fn timestamp(&mut self) -> Result<RuntimeValue> {
		let timestamp = self.ext.env_info().timestamp;
		Ok(RuntimeValue::from(timestamp))
	}
}

mod ext_impl {

	use wasmi::{Externals, RuntimeArgs, RuntimeValue, Error};
	use env::ids::*;

	macro_rules! void {
		{ $e: expr } => { { $e?; Ok(None) } }
	}

	macro_rules! some {
		{ $e: expr } => { { Ok(Some($e?)) } }
	}

	macro_rules! cast {
		{ $e: expr } => { { Ok(Some($e)) } }
	}

	impl<'a> Externals for super::Runtime<'a> {
		fn invoke_index(
			&mut self,
			index: usize,
			args: RuntimeArgs,
		) -> Result<Option<RuntimeValue>, Error> {
			match index {
				STORAGE_WRITE_FUNC => void!(self.storage_write(args)),
				STORAGE_READ_FUNC => void!(self.storage_read(args)),
				RET_FUNC => void!(self.ret(args)),
				GAS_FUNC => void!(self.gas(args)),
				INPUT_LENGTH_FUNC => cast!(self.input_legnth(args)),
				FETCH_INPUT_FUNC => void!(self.fetch_input(args)),
				PANIC_FUNC => void!(self.panic(args)),
				DEBUG_FUNC => void!(self.debug(args)),
				CCALL_FUNC => some!(self.ccall(args)),
				DCALL_FUNC => some!(self.dcall(args)),
				SCALL_FUNC => some!(self.scall(args)),
				VALUE_FUNC => void!(self.value(args)),
				CREATE_FUNC => some!(self.create(args)),
				SUICIDE_FUNC => void!(self.suicide(args)),
				BLOCKHASH_FUNC => void!(self.blockhash(args)),
				BLOCKNUMBER_FUNC => some!(self.blocknumber()),
				COINBASE_FUNC => void!(self.coinbase(args)),
				DIFFICULTY_FUNC => void!(self.difficulty(args)),
				GASLIMIT_FUNC => void!(self.gaslimit(args)),
				TIMESTAMP_FUNC => some!(self.timestamp()),
				_ => panic!("env module doesn't provide function at index {}", index),
			}
		}
	}
}