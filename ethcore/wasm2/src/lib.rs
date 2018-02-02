// Copyright 2015-2017 Parity Technologies (UK) Ltd.
// This file is part of Parity.

// Parity is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity.  If not, see <http://www.gnu.org/licenses/>.

//! Wasm Interpreter

extern crate byteorder;
extern crate ethcore_logger;
extern crate ethereum_types;
#[macro_use] extern crate log;
extern crate libc;
extern crate parity_wasm;
extern crate vm;
extern crate wasm_utils;
extern crate wasmi;

mod runtime;
// mod ptr;
// mod result;
#[cfg(test)]
mod tests;
mod env;
mod panic_payload;
mod parser;

use parity_wasm::elements;

use vm::{GasLeft, ReturnData, ActionParams};
use wasmi::Error as InterpreterError;

use runtime::{Runtime, RuntimeContext};

use ethereum_types::U256;

const MEMORY_STIPEND: u32 = 17;

/// Wrapped interpreter error
#[derive(Debug)]
pub struct Error(InterpreterError);

impl From<InterpreterError> for Error {
	fn from(e: InterpreterError) -> Self {
		Error(e)
	}
}

impl From<Error> for vm::Error {
	fn from(e: Error) -> Self {
		vm::Error::Wasm(format!("Wasm runtime error: {:?}", e.0))
	}
}

/// Wasm interpreter instance
pub struct WasmInterpreter;

impl From<runtime::Error> for vm::Error {
	fn from(e: runtime::Error) -> Self {
		vm::Error::Wasm(format!("Wasm runtime error: {:?}", e))
	}
}

impl vm::Vm for WasmInterpreter {

	fn exec(&mut self, params: ActionParams, ext: &mut vm::Ext) -> vm::Result<GasLeft> {
		let (module, data) = parser::payload(&params, ext.schedule())?;

		let loaded_module = wasmi::Module::from_parity_wasm_module(module).map_err(Error)?;

		let instantiation_resolover = env::ImportResolver::with_limit(64);

		let module_instance = wasmi::ModuleInstance::new(
			&loaded_module,
			&wasmi::ImportsBuilder::new().with_resolver("env", &instantiation_resolover)
		).map_err(Error)?;

		if params.gas > ::std::u64::MAX.into() {
			return Err(vm::Error::Wasm("Wasm interpreter cannot run contracts with gas >= 2^64".to_owned()));
		}

		let initial_memory = instantiation_resolover.memory_size().map_err(Error)?
			.saturating_sub(MEMORY_STIPEND);
		trace!(target: "wasm", "Contract requested {:?} pages of initial memory", initial_memory);

		let mut runtime = Runtime::with_params(
			ext,
			instantiation_resolover.memory_ref().map_err(Error)?,
			params.gas.low_u64(),
			data.to_vec(),
			RuntimeContext {
				address: params.address,
				sender: params.sender,
				origin: params.origin,
				code_address: params.code_address,
				value: params.value.value(),
			},
		);

		// cannot overflow if static_region < 2^16,
		// initial_memory ∈ [0..2^32)
		// total_charge <- static_region * 2^32 * 2^16
		// total_charge ∈ [0..2^64) if static_region ∈ [0..2^16)
		// qed
		assert!(runtime.schedule().wasm.static_region < 2^16);
		runtime.charge(|s| initial_memory as u64 * 64 * 1024 * s.wasm.static_region as u64)?;

		let module_instance = module_instance.run_start(&mut runtime).map_err(Error)?;

		module_instance.invoke_export("call", &[], &mut runtime).map_err(Error)?;

		if runtime.result().is_empty() {
			trace!(target: "wasm", "Contract execution result is empty.");
			Ok(GasLeft::Known(runtime.gas_left()?.into()))
		} else {
			let len = runtime.result().len();
			let gas_left: U256 = runtime.gas_left()?.into();
			Ok(GasLeft::NeedsReturn {
				gas_left: gas_left,
				data: ReturnData::new(
					runtime.dissolve(),
					0,
					len,
				),
				apply_state: true,
			})
		}
	}
}
