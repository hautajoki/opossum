// Prints TypeScript 5's getPropertiesOfType order for interfaces, classes, annotated variables and createGuard type
// arguments. Writes expected-ts5.txt for the property_order test:
//   node ts5props.cjs tsconfig.json src/ | grep -v globals.d.ts | { read h; echo "$h (ts5props.cjs, lines sorted)"; sort; } > expected-ts5.txt
// usage: node ts5props.cjs <tsconfig> <file substring> [--ts <typescript dir>] [--check-first]
"use strict";
const path = require("path");
const args = process.argv.slice(2);
const tsDir = args.includes("--ts")
	? args[args.indexOf("--ts") + 1]
	: "E:/Projects/TS/Flamework/roblox-ts-test/roblox-ts/node_modules/typescript";
const ts = require(tsDir);
const [tsconfig, filter] = args;
const checkFirst = args.includes("--check-first");
const config = ts.getParsedCommandLineOfConfigFile(path.resolve(tsconfig), {}, { ...ts.sys, onUnRecoverableConfigFileDiagnostic: (d) => { throw new Error(ts.flattenDiagnosticMessageText(d.messageText, "\n")); } });
const program = ts.createProgram({ rootNames: config.fileNames, options: config.options });
const checker = program.getTypeChecker();
console.log(`# typescript ${ts.version}`);
const files = program.getSourceFiles().filter((f) => f.fileName.includes(filter));
if (checkFirst) for (const f of program.getSourceFiles()) checker.getDiagnostics?.(f) ?? program.getSemanticDiagnostics(f);
const props = (type) => checker.getPropertiesOfType(type).map((p) => p.name).join(", ");
for (const file of files) {
	const rel = path.relative(path.dirname(path.resolve(tsconfig)), file.fileName).split(path.sep).join("/");
	const visit = (node) => {
		if ((ts.isInterfaceDeclaration(node) || ts.isClassDeclaration(node)) && node.name) {
			const sym = checker.getSymbolAtLocation(node.name);
			if (sym) console.log(`${rel} decl ${node.name.text}: ${props(checker.getDeclaredTypeOfSymbol(sym))}`);
		}
		if (ts.isVariableDeclaration(node) && node.type && ts.isTypeReferenceNode(node.type)) {
			console.log(`${rel} var ${node.name.getText()}: ${props(checker.getTypeAtLocation(node.name))}`);
		}
		if (ts.isCallExpression(node) && node.expression.getText().endsWith("createGuard") && node.typeArguments) {
			const type = checker.getTypeFromTypeNode(node.typeArguments[0]);
			const line = file.getLineAndCharacterOfPosition(node.getStart()).line + 1;
			console.log(`${rel}:${line} guard ${node.typeArguments[0].getText()}: ${props(type)}`);
		}
		ts.forEachChild(node, visit);
	};
	visit(file);
}
