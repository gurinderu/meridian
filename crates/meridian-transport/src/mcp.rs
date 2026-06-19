use serde_json::Value;

pub trait ToolRegistry: Send + Sync {
    fn list(&self) -> Vec<Value>;
    fn call(&self, name: &str, args: &Value) -> Value;
}
