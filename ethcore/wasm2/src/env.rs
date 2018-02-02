use std::cell::RefCell;
use wasmi::{
	self, Signature, Error, FuncRef, FuncInstance, MemoryDescriptor,
	MemoryRef, MemoryInstance,
};

pub mod ids {
	pub const STORAGE_READ_FUNC: usize = 10;
	pub const RET_FUNC: usize = 20;
	pub const GAS_FUNC: usize = 30;
	pub const FETCH_INPUT_FUNC: usize = 40;
	pub const INPUT_LENGTH_FUNC: usize = 50;
	pub const PANIC_FUNC: usize = 100;
}

pub mod signatures {
	use wasmi::{self, ValueType};
	use wasmi::ValueType::*;

	pub struct StaticSignature(pub &'static [ValueType], pub Option<ValueType>);

	pub const STORAGE_READ: StaticSignature = StaticSignature(
		&[I32, I32],
		None,
	);

	pub const RET: StaticSignature = StaticSignature(
		&[I32, I32],
		None,
	);

	pub const GAS: StaticSignature = StaticSignature(
		&[I32],
		None,
	);

	pub const FETCH_INPUT: StaticSignature = StaticSignature(
		&[I32],
		None,
	);

	pub const INPUT_LENGTH: StaticSignature = StaticSignature(
		&[],
		Some(I32),
	);

	pub const PANIC: StaticSignature = StaticSignature(
		&[I32, I32],
		None,
	);

	impl Into<wasmi::Signature> for StaticSignature {
		fn into(self) -> wasmi::Signature {
			wasmi::Signature::new(self.0, self.1)
		}
	}
}

fn host(signature: signatures::StaticSignature, idx: usize) -> FuncRef {
	FuncInstance::alloc_host(signature.into(), idx)
}

#[derive(Default)]
pub struct ImportResolver {
	max_memory: u32,
	memory: RefCell<Option<MemoryRef>>,
}

impl ImportResolver {
	pub fn with_limit(max_memory: u32) -> ImportResolver {
		ImportResolver {
			max_memory: max_memory,
			.. Default::default()
		}
	}

	pub fn memory_ref(&self) -> Result<MemoryRef, Error> {
		{
			let mut mem_ref = self.memory.borrow_mut();
			if mem_ref.is_none() { *mem_ref = Some(MemoryInstance::alloc(0, Some(0))?); }
		}

		Ok(self.memory.borrow().clone().expect("it is either existed or was created as (0, 0) above; qed"))
	}

	pub fn memory_size(&self) -> Result<u32, Error> {
		Ok(self.memory_ref()?.size())
	}
}

impl wasmi::ModuleImportResolver for ImportResolver {
	fn resolve_func(&self, field_name: &str, signature: &Signature) -> Result<FuncRef, Error> {
		let func_ref = match field_name {
			"storage_read" => host(signatures::STORAGE_READ, ids::STORAGE_READ_FUNC),
			"ret" => host(signatures::RET, ids::RET_FUNC),
			"gas" => host(signatures::GAS, ids::GAS_FUNC),
			"input_length" => host(signatures::INPUT_LENGTH, ids::INPUT_LENGTH_FUNC),
			"fetch_input" => host(signatures::FETCH_INPUT, ids::FETCH_INPUT_FUNC),
			"panic" => host(signatures::PANIC, ids::PANIC_FUNC),
			_ => {
				return Err(wasmi::Error::Instantiation(
					format!("Export {} not found", field_name),
				))
			}
		};

		Ok(func_ref)
	}

	fn resolve_memory(
		&self,
		field_name: &str,
		descriptor: &MemoryDescriptor,
	) -> Result<MemoryRef, Error> {
		if field_name == "memory" {
			if descriptor.initial() >= self.max_memory ||
				(descriptor.maximum().map_or(false, |m| m < descriptor.initial() || m > self.max_memory))
			{
				Err(Error::Instantiation("Module requested too much memory".to_owned()))
			} else {
				let mem = MemoryInstance::alloc(descriptor.initial(), descriptor.maximum())?;
				*self.memory.borrow_mut() = Some(mem.clone());
				Ok(mem)
			}
		} else {
			Err(Error::Instantiation("Memory imported under unknown name".to_owned()))
		}
	}
}