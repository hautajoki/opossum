//! `util/createGetService.ts`

use rbxts_luau::{Expr, create, globals};

/// `game:GetService(serviceName)`
pub fn create_get_service(service_name: &str) -> Expr {
    create::method_call(globals::game(), "GetService", vec![create::string(service_name)])
}
