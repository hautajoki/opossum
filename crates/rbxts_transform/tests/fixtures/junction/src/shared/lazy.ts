// A dynamic import of a linked package.
export async function load() {
	const greeter = await import("@rbxts/greeter");
	return greeter.greet("lazy");
}
