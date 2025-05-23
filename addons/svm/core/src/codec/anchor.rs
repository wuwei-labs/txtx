use std::path::PathBuf;

use crate::typing::anchor::types as anchor_types;

use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};
use txtx_addon_kit::{
    indexmap::IndexMap,
    types::{
        diagnostics::Diagnostic,
        types::{ObjectType, Value},
    },
};

use crate::typing::SvmValue;

use super::idl::IdlRef;

pub struct AnchorProgramArtifacts {
    /// The IDL of the anchor program, stored for an anchor project at `target/idl/<program_name>.json`.
    pub idl: anchor_types::Idl,
    /// The binary of the anchor program, stored for an anchor project at `target/deploy/<program_name>.so`.
    pub bin: Vec<u8>,
    /// The keypair of the anchor program, stored for an anchor project at `target/deploy/<program_name>-keypair.json`.
    pub keypair: Keypair,
    /// The program pubkey of the anchor program.
    pub program_id: Pubkey,
}

impl AnchorProgramArtifacts {
    pub fn new(
        keypair_path: PathBuf,
        idl_path: PathBuf,
        bin_path: PathBuf,
    ) -> Result<Self, String> {
        let idl_bytes = std::fs::read(&idl_path).map_err(|e| {
            format!("invalid anchor idl location {}: {}", &idl_path.to_str().unwrap_or(""), e)
        })?;

        let idl_ref = IdlRef::from_bytes(&idl_bytes).map_err(|e| {
            format!("invalid anchor idl at location {}: {}", &idl_path.to_str().unwrap_or(""), e)
        })?;

        let bin = std::fs::read(&bin_path).map_err(|e| {
            format!(
                "invalid anchor program binary location {}: {}",
                &bin_path.to_str().unwrap_or(""),
                e
            )
        })?;

        let keypair_file = std::fs::read(&keypair_path).map_err(|e| {
            format!(
                "invalid anchor program keypair location {}: {}",
                &bin_path.to_str().unwrap_or(""),
                e
            )
        })?;

        let keypair_bytes: Vec<u8> = serde_json::from_slice(&keypair_file).map_err(|e| {
            format!(
                "invalid anchor program keypair at location {}: {}",
                &keypair_path.to_str().unwrap_or(""),
                e
            )
        })?;

        let keypair = Keypair::from_bytes(&keypair_bytes).map_err(|e| {
            format!(
                "invalid anchor program keypair at location {}: {}",
                &keypair_path.to_str().unwrap_or(""),
                e
            )
        })?;

        if idl_ref.idl.address.ne(&keypair.pubkey().to_string()) {
            return Err(format!(
                "anchor idl address does not match keypair: idl specifies {}; keystore contains {}. Did you forget to run `anchor build`?",
                idl_ref.idl.address,
                keypair.pubkey().to_string()
            ));
        }
        let pubkey = keypair.pubkey();

        Ok(Self { idl: idl_ref.idl, bin, keypair, program_id: pubkey })
    }

    pub fn to_value(&self) -> Result<Value, String> {
        // let idl_bytes =
        //     serde_json::to_vec(&self.idl).map_err(|e| format!("invalid anchor idl: {e}"))?;

        let idl_str = serde_json::to_string_pretty(&self.idl)
            .map_err(|e| format!("invalid anchor idl: {e}"))?;

        let keypair_bytes = self.keypair.to_bytes();

        Ok(ObjectType::from(vec![
            ("binary", SvmValue::binary(self.bin.clone())),
            // ("idl", SvmValue::idl(idl_bytes)),
            ("idl", Value::string(idl_str)),
            ("keypair", SvmValue::keypair(keypair_bytes.to_vec())),
            ("program_id", Value::string(self.program_id.to_string())),
            ("framework", Value::string("anchor".to_string())),
        ])
        .to_value())
    }

    pub fn from_map(map: &IndexMap<String, Value>) -> Result<Self, Diagnostic> {
        let bin = match map.get("binary") {
            Some(Value::Addon(addon_data)) => addon_data.bytes.clone(),
            _ => return Err(diagnosed_error!("anchor artifacts missing binary")),
        };
        // let idl_bytes = match map.get("idl") {
        //     Some(Value::Addon(addon_data)) => addon_data.bytes.clone(),
        //     _ => return Err("anchor artifacts missing idl".to_string()),
        // };
        let idl_str =
            map.get("idl").ok_or(diagnosed_error!("anchor artifacts missing idl"))?.to_string();
        // let idl: Idl =
        //     serde_json::from_slice(&idl_bytes).map_err(|e| format!("invalid anchor idl: {e}"))?;

        let idl: anchor_types::Idl = serde_json::from_str(&idl_str)
            .map_err(|e| diagnosed_error!("invalid anchor idl: {e}"))?;

        let keypair_bytes = match map.get("keypair") {
            Some(Value::Addon(addon_data)) => addon_data.bytes.clone(),
            _ => return Err(diagnosed_error!("anchor artifacts missing keypair")),
        };
        let keypair = Keypair::from_bytes(&keypair_bytes)
            .map_err(|e| diagnosed_error!("invalid anchor keypair: {e}"))?;
        let pubkey = keypair.pubkey();
        Ok(Self { idl, bin, keypair, program_id: pubkey })
    }
}
