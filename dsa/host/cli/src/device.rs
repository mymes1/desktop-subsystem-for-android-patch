//! `dsa settings device`, `dsa settings set device.<field>` and `dsa settings reset device`:
//! the device identity Android reports to its apps.

use dsa_host_core::device_identity::{DeviceIdentity, Field};
use dsa_host_core::host_api::HostApiClient;
use serde_json::{json, Value};

use crate::Change;

/// `dsa settings set device.<field> <value>`.
pub fn parse_set(field: &str, value: &str) -> Result<Change, String> {
    let field = Field::parse(field)?;
    field.check(value)?;
    Ok(Change::Device(field, value.to_string()))
}

/// `dsa settings reset device` or `dsa settings reset device.<field>`.
pub fn parse_reset(name: &str) -> Result<Change, String> {
    if name == "device" {
        return Ok(Change::ResetDevice(None));
    }
    match name.strip_prefix("device.") {
        Some(field) => Ok(Change::ResetDevice(Some(Field::parse(field)?))),
        None => Err(format!(
            "dsa settings reset takes device or device.<field>, not {name:?}"
        )),
    }
}

/// An identity as one JSON object, with a key for every field.
pub fn identity_json(identity: &DeviceIdentity) -> Value {
    let mut object = serde_json::Map::new();
    for field in Field::ALL {
        object.insert(
            field.key().replace('.', "_"),
            Value::String(identity.get(field).to_string()),
        );
    }
    Value::Object(object)
}

/// What the user chose and what Android reports, a line for each field.
pub fn describe(chosen: &DeviceIdentity, effective: &DeviceIdentity) -> String {
    let mut lines: Vec<String> = Field::ALL
        .into_iter()
        .map(|field| {
            let value = effective.get(field);
            let value = if value.is_empty() {
                "(the image does not say)"
            } else {
                value
            };
            let source = if chosen.get(field).is_empty() {
                ""
            } else {
                " (changed)"
            };
            format!("{}: {value}{source}", field.key())
        })
        .collect();
    lines.push("Android reads these when it starts; a change applies from its next start.".into());
    lines.join("\n")
}

/// Shows the identity, after changing it as `change` says.
pub fn run(change: &Change, json: bool, client: &mut HostApiClient) -> Result<String, String> {
    let (mut chosen, mut effective) = client.device_identity().map_err(|e| e.to_string())?;
    let wanted = match change {
        Change::Device(field, value) => {
            let mut wanted = chosen.clone();
            wanted.set(*field, value)?;
            Some(wanted)
        }
        Change::ResetDevice(Some(field)) => {
            let mut wanted = chosen.clone();
            wanted.reset(*field);
            Some(wanted)
        }
        Change::ResetDevice(None) => Some(DeviceIdentity::default()),
        _ => None,
    };
    if let Some(wanted) = wanted {
        (chosen, effective) = client
            .set_device_identity(&wanted)
            .map_err(|e| e.to_string())?;
    }
    Ok(if json {
        json!({
            "chosen": identity_json(&chosen),
            "effective": identity_json(&effective),
        })
        .to_string()
    } else {
        describe(&chosen, &effective)
    })
}
