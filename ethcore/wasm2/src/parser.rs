use vm;
use wasm_utils::{self, rules};
use parity_wasm::elements::{self, Deserialize};
use parity_wasm::peek_size;

fn gas_rules(schedule: &vm::Schedule) -> rules::Set {
	rules::Set::new({
		let mut vals = ::std::collections::HashMap::with_capacity(4);
		vals.insert(rules::InstructionType::Load, schedule.wasm.mem as u32);
		vals.insert(rules::InstructionType::Store, schedule.wasm.mem as u32);
		vals.insert(rules::InstructionType::Div, schedule.wasm.div as u32);
		vals.insert(rules::InstructionType::Mul, schedule.wasm.mul as u32);
		vals
	})
}

/// Splits payload to code and data according to params.params_type
/// Panics if params.code is None!
pub fn payload<'a>(params: &'a vm::ActionParams, schedule: &vm::Schedule)
	-> Result<(elements::Module, &'a [u8]), vm::Error>
{
	let code = match params.code {
		Some(ref code) => &code[..],
		None => { return Err(vm::Error::Wasm("Invalid wasm call".to_owned())); }
	};

	let (mut cursor, data_position) = match params.params_type {
		vm::ParamsType::Embedded => {
			let module_size = peek_size(&*code);
			(
				::std::io::Cursor::new(&code[..module_size]),
				module_size
			)
		},
		vm::ParamsType::Separate => {
			(::std::io::Cursor::new(&code[..]), 0)
		},
	};

	let contract_module = wasm_utils::inject_gas_counter(
		elements::Module::deserialize(
			&mut cursor
		).map_err(|err| {
			vm::Error::Wasm(format!("Error deserializing contract code ({:?})", err))
		})?,
		&gas_rules(schedule),
	);

	let data = match params.params_type {
		vm::ParamsType::Embedded => {
			if data_position < code.len() { &code[data_position..] } else { &[] }
		},
		vm::ParamsType::Separate => {
			match params.data {
				Some(ref s) => &s[..],
				None => &[]
			}
		}
	};

	Ok((contract_module, data))
}