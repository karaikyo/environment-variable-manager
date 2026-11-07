#![windows_subsystem = "windows"]
//! Developer-only fixture executable, excluded from the portable product.
use environment_variable_manager::{desktop, RegistryReader, RegistryWriter, Scope, Variable};
struct Fixture;
impl RegistryWriter for Fixture {
    fn write_user(&self, _: &Variable) -> Result<(), String> {
        Err("Developer browse fixture does not apply writes".into())
    }
}
impl RegistryReader for Fixture {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(vec![
            Variable::text(scope, "", "", false),
            Variable::text(
                scope,
                "MiXeD",
                " alpha;;comma,value;dup;dup;%SystemRoot%;embedded\r\nnewline;",
                true,
            ),
            Variable {
                scope,
                name: "Unsupported".into(),
                registry_type: 3,
                bytes: vec![0, 255, 1],
            },
        ])
    }
}
fn main() {
    let data = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("data");
    if desktop::run(Fixture, data).is_err() {
        std::process::exit(1);
    }
}
