use wasmi::{self, Signature, Error, FuncRef, FuncInstance};

pub mod ids {
	pub const STORAGE_READ_FUNC: usize = 10;
}

pub mod signatures {
	use wasmi::{self, ValueType};
	use wasmi::ValueType::*;

	pub struct StaticSignature(pub &'static [ValueType], pub Option<ValueType>);

	pub const STORAGE_READ: StaticSignature = StaticSignature(
		&[I32, I32],
		None
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

pub struct ImportResolver;

impl wasmi::ModuleImportResolver for ImportResolver {
	fn resolve_func(&self, field_name: &str, signature: &Signature) -> Result<FuncRef, Error> {
		let func_ref = match field_name {
			"storage_read" => { host(signatures::STORAGE_READ, ids::STORAGE_READ_FUNC) },
			_ => {
				return Err(wasmi::Error::Instantiation(
					format!("Export {} not found", field_name),
				))
			}
		};

		Ok(func_ref)
	}
}