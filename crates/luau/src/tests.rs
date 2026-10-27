//! Renderer edge cases: precedence, string quoting, long brackets, temporary names, requires.

use crate::create::*;
use crate::*;

fn print(args: Vec<Expr>) -> Stmt {
    call_statement(call(id("print"), args))
}

fn render_lines(statements: Vec<Stmt>) -> String {
    render_ast(&statements)
}

fn b(left: Expr, op: &str, right: Expr) -> Expr {
    binary(left, BinaryOperator::parse(op).unwrap(), right)
}

fn u(op: &str, e: Expr) -> Expr {
    unary(UnaryOperator::parse(op).unwrap(), e)
}

#[test]
fn trees_are_send() {
    fn send<T: Send + Sync>() {}
    send::<Stmt>();
    send::<Expr>();
}

#[test]
fn string_quotes_and_long_brackets() {
    let values = ["plain", "say \"hi\"", "it's", "both \" and '", "two\nlines", "a]]b\nc", "ends]", "x]=]y\nz"];
    let mut statements: Vec<Stmt> = values.iter().map(|v| print(vec![string(*v)])).collect();
    statements.push(print(vec![
        string_quoted("already \\'escaped\\'", Quote::Single),
        string_quoted("dq \\\"", Quote::Double),
    ]));
    assert_eq!(
        render_lines(statements),
        "print(\"plain\")\nprint('say \"hi\"')\nprint(\"it's\")\nprint([[both \" and ']])\nprint([[two\nlines]])\n\
         print([=[a]]b\nc]=])\nprint(\"ends]\")\nprint([[x]=]y\nz]])\nprint('already \\'escaped\\'', \"dq \\\"\")\n"
    );
}

#[test]
fn long_brackets_inside_index_brackets_get_spaces() {
    let statements = vec![
        print(vec![map([(string("a\nb"), number(1.0)), (string("ok"), number(2.0)), (string("not ok"), number(3.0))])]),
        print(vec![
            computed_index(id("t"), string("x\ny")),
            computed_index(id("t"), string("name")),
            computed_index(id("t"), string("end")),
        ]),
        print(vec![set(vec![string("k\nl"), string("key"), number(5.0)])]),
        print(vec![property(id("t"), "my-prop")]),
    ];
    assert_eq!(
        render_lines(statements),
        "print({\n\t[ [[a\nb]] ] = 1,\n\tok = 2,\n\t[\"not ok\"] = 3,\n})\nprint(t[ [[x\ny]] ], t.name, t[\"end\"])\n\
         print({\n\t[ [[k\nl]] ] = true,\n\tkey = true,\n\t[5] = true,\n})\nprint(t[\"my-prop\"])\n"
    );
}

#[test]
fn number_literals() {
    let texts = |ts: &[&str]| print(ts.iter().map(|t| number_text(*t)).collect());
    let statements = vec![
        texts(&["1_000", "0x_FF", "0b1010", ".5", "1e10", "5."]),
        texts(&["0o17", "Infinity", "1_0n", "0O7_7"]),
        print(vec![number(-5.0), number(1e21), number(0.1 + 0.2), number(-0.0), number(1e-7), number(123456789.125)]),
        print(vec![u("-", number(-5.0)), u("-", u("-", id("a")))]),
    ];
    assert_eq!(
        render_lines(statements),
        "print(1_000, 0x_FF, 0b1010, .5, 1e10, 5.)\nprint(15, Infinity, NaN, 63)\n\
         print(-5, 1e+21, 0.30000000000000004, 0, 1e-7, 123456789.125)\nprint(- -5, - -a)\n"
    );
}

#[test]
fn precedence_and_parentheses() {
    let (a, bb, c, d) = (|| id("a"), || id("b"), || id("c"), || id("d"));
    let statements = vec![
        print(vec![b(b(a(), "+", bb()), "*", c()), b(a(), "-", b(bb(), "-", c())), b(b(a(), "-", bb()), "-", c())]),
        print(vec![b(a(), "^", b(bb(), "^", c())), b(u("-", a()), "^", bb()), b(a(), "^", u("-", bb()))]),
        print(vec![u("not", b(a(), "==", bb())), u("-", b(a(), "+", bb())), u("#", computed_index(a(), number(1.0)))]),
        print(vec![
            b(b(a(), "and", bb()), "or", c()),
            b(b(a(), "or", bb()), "and", c()),
            b(a(), "or", b(bb(), "and", c())),
        ]),
        print(vec![
            b(a(), "..", b(bb(), "..", c())),
            b(b(a(), "..", bb()), "..", c()),
            b(b(a(), "//", bb()), "%", c()),
        ]),
        // an if-expression is parenthesized as any binary operand: roblox-ts left it bare on the left of `or`, which
        // then joined its `else` branch
        print(vec![
            b(if_expr(a(), bb(), c()), "+", d()),
            b(d(), "or", if_expr(a(), bb(), c())),
            b(if_expr(a(), bb(), c()), "or", d()),
        ]),
        print(vec![u("not", u("not", a())), u("-", if_expr(a(), bb(), c()))]),
        print(vec![paren(paren(a())), paren(b(a(), "+", bb())), b(paren(b(a(), "+", bb())), "*", c())]),
    ];
    assert_eq!(
        render_lines(statements),
        "print((a + b) * c, a - (b - c), a - b - c)\nprint(a ^ (b ^ c), (-a) ^ b, a ^ (-b))\n\
         print(not (a == b), -(a + b), #a[1])\nprint(a and b or c, (a or b) and c, a or b and c)\n\
         print(a .. (b .. c), a .. b .. c, a // b % c)\n\
         print((if a then b else c) + d, d or (if a then b else c), (if a then b else c) or d)\n\
         print(not not a, -(if a then b else c))\nprint(a, (a + b), (a + b) * c)\n"
    );
}

#[test]
fn semicolons_before_a_parenthesis() {
    let statements = vec![
        local(any_id("a"), Some(id("b"))),
        call_statement(call(paren(function(vec![], false, vec![])), vec![])),
        assignment(id("x"), AssignmentOperator::Assign, number(1.0)),
        // `(f)()` renders as `f()` but still counts as starting with a parenthesis
        call_statement(call(paren(id("f")), vec![])),
        call_statement(call(id("g"), vec![])),
        comment(" a comment in between"),
        assignment(computed_index(paren(id("t")), number(1.0)), AssignmentOperator::Assign, number(2.0)),
        local(any_id("c"), None),
        call_statement(method_call(paren(id("obj")), "m", vec![])),
        local(any_id("d"), Some(b(id("x"), "+", number(1.0)))),
        call_statement(call(paren(id("h")), vec![])),
        Stmt::Assignment(Box::new(Assignment {
            left: vec![id("p"), id("q")],
            operator: AssignmentOperator::Assign,
            right: vec![id("q"), u("-", id("p"))],
        })),
        call_statement(call(paren(id("i")), vec![])),
        local(any_id("e"), Some(if_expr(id("x"), number(1.0), id("y")))),
        Stmt::Assignment(Box::new(Assignment {
            left: vec![property(paren(id("u")), "v"), id("w")],
            operator: AssignmentOperator::Assign,
            right: vec![number(1.0), number(2.0)],
        })),
    ];
    assert_eq!(
        render_lines(statements),
        "local a = b;\n(function() end)()\nx = 1\nf()\ng();\n-- a comment in between\nt[1] = 2\nlocal c;\nobj:m()\n\
         local d = x + 1\nh()\np, q = q, -p;\ni()\nlocal e = if x then 1 else y;\nu.v, w = 1, 2\n"
    );
}

#[test]
fn temporary_identifier_names() {
    let mut ids = TempIds::new();
    let exp = ids.temp_id("exp");
    let exp2 = ids.temp_id("exp");
    let anon = ids.temp_id("");
    let anon2 = ids.temp_id("");
    let x1 = ids.temp_id("x");
    let x2 = ids.temp_id("x");
    let x_one = ids.temp_id("x_1");
    let inner = ids.temp_id("exp");
    let param = ids.temp_id("param");
    let for_only = ids.temp_id("value");
    let for_only2 = ids.temp_id("other");
    let statements = vec![
        local(any_id("_exp"), Some(number(0.0))),
        local(exp.clone(), Some(number(1.0))),
        local(exp2, Some(number(2.0))),
        locals(vec![anon.into(), anon2.into()], vec![number(3.0), number(4.0)]),
        local(x1, Some(number(5.0))),
        local(x2, Some(number(6.0))),
        // TypeScript's `lastTry` is keyed by the name it settled on: `_x_1` remembers 2, so this is `_x_1_2`
        local(x_one, Some(number(7.0))),
        function_declaration(
            true,
            id("f"),
            vec![param.clone().into(), any_id("_exp_2")],
            true,
            vec![
                local(inner.clone(), Some(param.into())),
                Stmt::Return(ReturnStatement { expressions: vec![exp.clone().into(), inner.into(), var_args()] }),
            ],
        ),
        // the `in` expression is not walked by the solver: these get the fallback names, past every name of the tree
        // (roblox-ts gave `_1`, which read the `local _1` above)
        for_in(
            vec![any_id("_"), any_id("v")],
            call(id("pairs"), vec![map([(for_only2.into(), number(1.0)), (string("k"), for_only.into())])]),
            vec![print(vec![id("v")])],
        ),
        do_stmt(vec![local(ids.temp_id("exp"), Some(number(8.0))), print(vec![exp.into()])]),
    ];
    assert_eq!(
        render_lines(statements),
        "local _exp = 0\nlocal _exp_1 = 1\nlocal _exp_2 = 2\nlocal _, _1 = 3, 4\nlocal _x = 5\nlocal _x_1 = 6\n\
         local _x_1_2 = 7\nlocal function f(_param, _exp_2, ...)\n\tlocal _exp_3 = _param\n\treturn _exp_1, _exp_3, ...\n\
         end\nfor _, v in pairs({\n\t[_0] = 1,\n\tk = _2,\n}) do\n\tprint(v)\nend\ndo\n\tlocal _exp_3 = 8\n\tprint(_exp_1)\nend\n"
    );
}

#[test]
fn temporary_identifier_scopes() {
    // a function declaration first in its block, and a temporary named after a local declared later in the block:
    // the parameter sees the later `local _t`, and the block does not see the parameter
    let mut ids = TempIds::new();
    let t = ids.temp_id("t");
    let statements = vec![do_stmt(vec![
        function_declaration(true, id("g"), vec![ids.temp_id("t").into()], false, vec![]),
        local(t.clone(), Some(number(1.0))),
        local(any_id("_t"), Some(t.into())),
    ])];
    assert_eq!(
        render_lines(statements),
        "do\n\tlocal function g(_t_1)\n\tend\n\tlocal _t_1 = 1\n\tlocal _t = _t_1\nend\n"
    );
}

#[test]
fn temporary_names_never_capture_or_hide_the_program_names() {
    // roblox-ts named every temporary here as the program's own identifier
    let mut ids = TempIds::new();
    let flag = ids.temp_id("flag");
    let g = ids.temp_id("g");
    let v = ids.temp_id("v");
    let unrelated = ids.temp_id("w");
    let k = ids.temp_id("k");
    let v2 = ids.temp_id("v");
    let statements = vec![
        // a declaration inside the temporary's scope that its use would see: `_flag = true` would set the user's
        do_stmt(vec![
            local(flag.clone(), Some(Expr::False)),
            if_stmt(
                id("x"),
                vec![
                    local(any_id("_flag"), Some(string("user"))),
                    assignment(flag.clone().into(), AssignmentOperator::Assign, Expr::True),
                ],
                ElseBody::default(),
            ),
            print(vec![flag.into()]),
        ]),
        // a use of a global (or of an outer local) inside the temporary's scope: it would read the temporary
        do_stmt(vec![local(g.clone(), Some(number(1.0))), print(vec![g.into(), id("_g")])]),
        // a loop variable in scope
        for_in(
            vec![any_id("_v")],
            id("list"),
            vec![local(v.clone(), Some(number(2.0))), print(vec![v.into(), id("_v")])],
        ),
        // a declaration the temporary's uses never see, and a use that refers to that declaration: no collision
        do_stmt(vec![
            local(unrelated.clone(), Some(number(3.0))),
            do_stmt(vec![local(any_id("_w"), Some(number(4.0))), print(vec![id("_w")])]),
            print(vec![unrelated.into()]),
        ]),
        // a local of the same name declared after the temporary's last use, and a loop variable of the same name
        // after it in the list: neither hides it
        for_in(
            vec![k.clone().into(), v2.clone().into()],
            id("m"),
            vec![local(any_id("_v"), Some(array(vec![k.into(), v2.into()]))), print(vec![id("_v")])],
        ),
        for_in(vec![ids.temp_id("").into(), any_id("_")], id("list"), vec![print(vec![id("_")])]),
    ];
    assert_eq!(
        render_lines(statements),
        "do\n\tlocal _flag_1 = false\n\tif x then\n\t\tlocal _flag = \"user\"\n\t\t_flag_1 = true\n\tend\n\tprint(_flag_1)\n\
         end\ndo\n\tlocal _g_1 = 1\n\tprint(_g_1, _g)\nend\nfor _v in list do\n\tlocal _v_1 = 2\n\tprint(_v_1, _v)\nend\n\
         do\n\tlocal _w = 3\n\tdo\n\t\tlocal _w = 4\n\t\tprint(_w)\n\tend\n\tprint(_w)\nend\n\
         for _k, _v in m do\n\tlocal _v = { _k, _v }\n\tprint(_v)\nend\nfor _, _ in list do\n\tprint(_)\nend\n"
    );
}

#[test]
fn reserved_program_names_are_renamed_without_capture() {
    // a user `next` (renamed) beside the global `next` the compiler calls, and a user `next_` already in scope
    let mut ids = TempIds::new();
    let next = ids.renamed("next");
    let end = ids.renamed("end");
    let statements = vec![
        local(any_id("next_"), Some(number(0.0))),
        local(next.clone(), Some(number(1.0))),
        function_declaration(true, end.clone().into(), vec![], false, vec![]),
        print(vec![next.into(), id("next_"), call(id("next"), vec![id("t")]), call(end.into(), vec![])]),
    ];
    assert_eq!(
        render_lines(statements),
        "local next_ = 0\nlocal next_1 = 1\nlocal function end_()\nend\nprint(next_1, next_, next(t), end_())\n"
    );
}

#[test]
fn blocks_and_empty_bodies() {
    let statements = vec![
        do_stmt(vec![]),
        while_stmt(bool(true), vec![break_stmt()]),
        repeat_stmt(vec![continue_stmt(), comment("after continue")], id("done")),
        if_stmt(
            id("a"),
            vec![],
            ElseBody::If(Box::new(IfStatement {
                condition: id("b"),
                statements: vec![print(vec![number(1.0)])],
                else_body: ElseBody::If(Box::new(IfStatement {
                    condition: id("c"),
                    statements: vec![],
                    else_body: ElseBody::Block(vec![print(vec![number(2.0)])]),
                })),
            })),
        ),
        if_stmt(id("d"), vec![print(vec![number(3.0)])], ElseBody::default()),
        numeric_for(any_id("i"), number(1.0), id("n"), Some(number_text("1.0")), vec![]),
        numeric_for(any_id("j"), number(10.0), number(1.0), Some(number(-1.0)), vec![print(vec![id("j")])]),
        for_in(vec![], call(id("iter"), vec![]), vec![]),
        local(any_id("empty"), Some(function(vec![], false, vec![]))),
        local(any_id("tables"), Some(array(vec![array(vec![]), map([]), set(vec![]), mixed_table(vec![])]))),
        method_declaration(
            id("Class"),
            "method",
            vec![any_id("self")],
            false,
            vec![return_stmt(mixed_table(vec![
                TableField::Value(number(1.0)),
                TableField::Field(MapField {
                    index: string("key"),
                    value: function(vec![any_id("x")], false, vec![return_stmt(id("x"))]),
                }),
                TableField::Field(MapField { index: number(2.0), value: number(3.0) }),
            ]))],
        ),
        function_declaration(
            false,
            property(id("Class"), "static"),
            vec![],
            true,
            vec![Stmt::Return(ReturnStatement { expressions: vec![] })],
        ),
    ];
    assert_eq!(
        render_lines(statements),
        "do\nend\nwhile true do\n\tbreak\nend\nrepeat\n\tcontinue\n\t--after continue\nuntil done\nif a then\n\
         elseif b then\n\tprint(1)\nelseif c then\nelse\n\tprint(2)\nend\nif d then\n\tprint(3)\nend\nfor i = 1, n do\nend\n\
         for j = 10, 1, -1 do\n\tprint(j)\nend\nfor _ in iter() do\nend\nlocal empty = function() end\n\
         local tables = { {}, {}, {}, {} }\nfunction Class:method(self)\n\treturn {\n\t\t1,\n\t\tkey = function(x)\n\
         \t\t\treturn x\n\t\tend,\n\t\t[2] = 3,\n\t}\nend\nfunction Class.static(...)\n\treturn \nend\n"
    );
}

#[test]
fn interpolated_strings() {
    let statements = vec![print(vec![interpolated_string(vec![
        InterpolatedStringPart::Text("a {b} \\{c\\} \\u{1F600} \\\\u{41}\nline\r\nend\\".into()),
        InterpolatedStringPart::Expression(array(vec![number(1.0)])),
        InterpolatedStringPart::Text(" and ".into()),
        InterpolatedStringPart::Expression(b(id("x"), "+", number(1.0))),
        InterpolatedStringPart::Expression(map([(string("k"), number(1.0))])),
    ])])];
    // a trailing backslash in a text part escapes the next expression's brace (as in TypeScript)
    assert_eq!(
        render_lines(statements),
        "print(`a \\{b\\} \\{c\\} \\u{1F600} \\\\u\\{41\\}\\\nline\\\r\nend\\{({ 1 })} and {x + 1}{({\n\tk = 1,\n})}`)\n"
    );
}

#[test]
fn if_expressions() {
    let statements = vec![
        local(
            any_id("v"),
            Some(if_expr(
                id("a"),
                number(1.0),
                if_expr(id("b"), number(2.0), if_expr(id("c"), number(3.0), number(4.0))),
            )),
        ),
        local(any_id("w"), Some(if_expr(if_expr(id("a"), id("b"), id("c")), id("d"), id("e")))),
    ];
    assert_eq!(
        render_lines(statements),
        "local v = if a then 1 elseif b then 2 elseif c then 3 else 4\nlocal w = if if a then b else c then d else e\n"
    );
}

#[test]
fn compound_assignments() {
    let ops = ["=", "+=", "-=", "*=", "/=", "//=", "%=", "^=", "..="];
    let mut statements: Vec<Stmt> =
        ops.iter().map(|op| assignment(id("a"), AssignmentOperator::parse(op).unwrap(), number(1.0))).collect();
    statements.push(assignment(computed_index(id("t"), string("k")), AssignmentOperator::Concat, string("x")));
    assert_eq!(
        render_lines(statements),
        "a = 1\na += 1\na -= 1\na *= 1\na /= 1\na //= 1\na %= 1\na ^= 1\na ..= 1\nt.k ..= \"x\"\n"
    );
}

#[test]
fn method_calls_on_literals() {
    // parentheses around a simple expression are dropped (as in TypeScript), but not around a literal that is
    // indexed or called (rbxts-rs: roblox-ts renders `"abc":upper()`, which does not parse)
    let statements = vec![
        print(vec![
            method_call(paren(string("abc")), "upper", vec![]),
            method_call(paren(number(5.0)), "x", vec![]),
            method_call(paren(array(vec![])), "y", vec![]),
        ]),
        print(vec![
            method_call(paren(b(id("a"), "..", id("b"))), "len", vec![]),
            call(paren(function(vec![], false, vec![return_stmt(number(1.0))])), vec![]),
        ]),
        print(vec![call(property(paren(map([(string("a"), number(1.0))])), "a"), vec![])]),
    ];
    assert_eq!(
        render_lines(statements),
        "print((\"abc\"):upper(), (5):x(), ({}):y())\nprint((a .. b):len(), (function()\n\treturn 1\nend)())\n\
         print(({\n\ta = 1,\n}).a())\n"
    );
}

#[test]
fn comments() {
    let statements = vec![
        comment("single"),
        comment("multi\n\n  \nline ]] and ]=]"),
        do_stmt(vec![comment("nested\nblock"), comment("")]),
    ];
    assert_eq!(
        render_lines(statements),
        "--single\n--[==[\n\tmulti\n\n\n\tline ]] and ]=]\n]==]\ndo\n\t--[[\n\t\tnested\n\t\tblock\n\t]]\n\t--\nend\n"
    );
}

#[test]
fn requires_are_spelled_from_the_module() {
    let path = |p: &str, init| ModulePath { names: p.split('/').map(str::to_owned).collect(), root: 2, init };
    let statements = vec![
        local(any_id("a"), Some(require(path("ServerScriptService/server/x/a", false)))),
        local(any_id("b"), Some(property(require(path("ReplicatedStorage/shared/b", false)), "b"))),
    ];
    assert_eq!(
        render(&statements, &path("ServerScriptService/server/main", false)),
        "local a = require(\"./x/a\")\nlocal b = require(\"@game/ReplicatedStorage/shared/b\").b\n"
    );
}

#[test]
#[should_panic(expected = "Cannot render statement after break, continue, or return!")]
fn statement_after_return_panics() {
    render_ast(&[return_stmt(nil()), comment("fine"), print(vec![])]);
}

#[test]
#[should_panic(expected = "Invalid Luau Identifier: \"end\"")]
fn invalid_identifier_panics() {
    render_ast(&[print(vec![id("end")])]);
}

#[test]
fn globals_and_guards() {
    use crate::globals;
    let statements = vec![
        print(vec![globals::ts(), globals::string::r#match(), globals::table::r#move(), globals::self_()]),
        print(vec![globals::strings::comma_space(), globals::coroutine::r#yield()]),
    ];
    assert_eq!(render_lines(statements), "print(TS, string.match, table.move, self)\nprint(\", \", coroutine.yield)\n");
    assert!(id("a").is_indexable() && paren(number(1.0)).is_indexable() && !number(1.0).is_indexable());
    assert!(string("s").is_simple_primitive() && !id("a").is_simple_primitive() && id("a").is_simple());
    assert!(array(vec![]).is_table() && Stmt::Break.is_final() && !comment("x").is_final());
}

#[test]
fn numbers_without_literals() {
    use crate::{create, render_expr};
    let text = |v: f64| render_expr(&create::number(v));
    assert_eq!(text(f64::NAN), "0 / 0");
    assert_eq!(text(f64::INFINITY), "math.huge");
    assert_eq!(text(f64::NEG_INFINITY), "-math.huge");
    assert_eq!(text(-1.5), "-1.5");
}

#[test]
fn statement_lines() {
    let statements = vec![
        crate::create::comment("a"),
        crate::create::function_declaration(
            true,
            crate::create::id("f"),
            vec![],
            false,
            vec![crate::create::call_statement(crate::create::call(crate::create::id("g"), vec![]))],
        ),
        crate::create::local(crate::create::any_id("x"), Some(crate::create::number(1.0))),
    ];
    let module = ModulePath { names: vec!["ReplicatedStorage".into(), "m".into()], root: 1, init: false };
    let (luau, lines) = crate::render_with_lines(&statements, &module);
    assert_eq!(luau, crate::render_ast(&statements));
    assert_eq!(luau, "--a\nlocal function f()\n\tg()\nend\nlocal x = 1\n");
    assert_eq!(lines, vec![1, 2, 5]);
}

#[test]
fn const_locals_follow_luau_scoping_and_every_write() {
    let local = |name: &str, value: Expr| crate::create::local(any_id(name), Some(value));
    let set = |name: &str, op: &str, value: Expr| assignment(id(name), AssignmentOperator::parse(op).unwrap(), value);
    let mut statements = vec![
        // shadowed: only the inner `a` is written
        local("a", number(1.0)),
        do_stmt(vec![local("a", number(2.0)), set("a", "=", number(3.0))]),
        // written from a closure, by a compound assignment, by a global-style function declaration
        local("b", number(1.0)),
        local("f", function(Vec::new(), false, vec![set("b", "+=", number(1.0))])),
        local("c", string("x")),
        set("c", "..=", string("y")),
        local("g", nil()),
        function_declaration(false, id("g"), Vec::new(), false, Vec::new()),
        // a `repeat` body's local is visible in its `until`; Luau needs a value per name
        repeat_stmt(vec![local("r", number(1.0))], id("r")),
        locals(vec![any_id("p"), any_id("q")], vec![number(1.0)]),
        locals(vec![any_id("s"), any_id("t")], vec![call(id("pair"), Vec::new())]),
        print(vec![id("a"), id("f"), id("c"), id("g"), id("p"), id("q"), id("s"), id("t")]),
    ];
    assert_eq!(mark_const_locals(&mut statements), 4);
    let text = render_lines(statements);
    assert_eq!(
        text,
        "const a = 1\ndo\n\tlocal a = 2\n\ta = 3\nend\nlocal b = 1\nconst f = function()\n\tb += 1\nend\nlocal c = \"x\"\n\
         c ..= \"y\"\nlocal g = nil\nfunction g()\nend\nrepeat\n\tconst r = 1\nuntil r\nlocal p, q = 1\nconst s, t = pair()\n\
         print(a, f, c, g, p, q, s, t)\n"
    );
    crate::gate::compile(&text, 2, 1).unwrap();
}

#[test]
fn types_parenthesize_and_quote() {
    use crate::create::{any_id, function_declaration, id, local, type_alias};
    let n = Type::named;
    let callback = Type::Function(Box::new(FunctionType {
        parameters: vec![(Some("x".into()), n("number"))],
        returns: TypePack::one(n("string")),
        ..FunctionType::default()
    }));
    let shape = Type::Table(Box::new(TableType {
        fields: vec![
            TableTypeField { name: "then".into(), ty: Type::String("a\"b".into()), read: true },
            TableTypeField { name: "list".into(), ty: Type::array(n("T")), read: false },
        ],
        indexer: None,
    }));
    let alias = type_alias(
        true,
        "Shape",
        vec![GenericParameter { name: "T".into(), default: Some(n("number")) }],
        Type::Union(vec![
            Type::Optional(Box::new(callback.clone())),
            Type::Intersection(vec![shape, Type::Union(vec![Type::Bool(true), n("T")])]),
        ]),
    );
    let mut f = function_declaration(true, id("f"), vec![any_id("a")], true, vec![]);
    if let Stmt::FunctionDeclaration(f) = &mut f {
        f.signature = Some(Box::new(Signature {
            generics: vec!["T".into()],
            parameters: vec![Some(callback)],
            var_args: Some(n("T")),
            returns: Some(TypePack { types: vec![n("T")], variadic: Some(Box::new(n("any"))) }),
        }));
    }
    let mut x = local(any_id("x"), None);
    if let Stmt::VariableDeclaration(x) = &mut x {
        x.types = vec![Some(Type::array(Type::Optional(Box::new(n("number")))))];
    }
    let luau = render_lines(vec![alias, f, x]);
    assert_eq!(
        luau,
        "export type Shape<T = number> = ((x: number) -> string)? | ({ read [\"then\"]: \"a\\\"b\", list: {T} } & (true | T))\n\
         local function f<T>(a: (x: number) -> string, ...: T): (T, ...any)\nend\nlocal x: {number?}\n"
    );
    crate::gate::compile(&luau, 2, 1).expect("typed luau compiles");
}
