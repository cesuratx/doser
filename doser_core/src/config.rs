use std::collections::HashMap;
use toml::Value;

pub trait ConfigSource {
    fn get_str(&self, key: &str) -> Option<String>;
    fn get_u8(&self, key: &str) -> Option<u8> {
        self.get_str(key).and_then(|v| v.parse().ok())
    }
    fn get_u32(&self, key: &str) -> Option<u32> {
        self.get_str(key).and_then(|v| v.parse().ok())
    }
}

pub struct TomlConfigSource(pub Value);
impl ConfigSource for TomlConfigSource {
    fn get_str(&self, key: &str) -> Option<String> {
        let v = match key {
            "hx711_dt_pin" => self
                .0
                .get("hx711")?
                .get("dt_pin")?
                .as_integer()
                .map(|v| v.to_string()),
            "hx711_sck_pin" => self
                .0
                .get("hx711")?
                .get("sck_pin")?
                .as_integer()
                .map(|v| v.to_string()),
            "stepper_step_pin" => self
                .0
                .get("stepper")?
                .get("step_pin")?
                .as_integer()
                .map(|v| v.to_string()),
            "stepper_dir_pin" => self
                .0
                .get("stepper")?
                .get("dir_pin")?
                .as_integer()
                .map(|v| v.to_string()),
            "max_attempts" => self
                .0
                .get("max_attempts")?
                .as_integer()
                .map(|v| v.to_string()),
            _ => None,
        };
        v
    }
}

pub struct CsvConfigSource(pub HashMap<String, String>);
impl ConfigSource for CsvConfigSource {
    fn get_str(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}
