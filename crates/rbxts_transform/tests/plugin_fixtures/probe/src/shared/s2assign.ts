interface Attrs {
	count: number;
	label: string;
	flag?: boolean;
}

// the assignment hook: `this.attrs.<key> …` becomes `self:setAttribute("<key>", value[, true])`
export class Comp {
	/** @probeattrs */
	attrs: Attrs = { count: 0, label: "" };
	plain = { count: 0 };

	run() {
		this.attrs.count = 1;
		this.attrs.count += 2;
		this.attrs.label += "x";
		this.attrs.count++;
		const a = this.attrs.count++;
		const b = ++this.attrs.count;
		const c = (this.attrs.count = 5);
		this.attrs.flag ??= true;
		this.attrs["count"] -= sideEffect();
		delete this.attrs.flag;
		this.plain.count = 1;
		for (let i = 0; i < 2; this.attrs.count++) {
			print(i);
		}
		// claimed inside parentheses of a `for` head: the parentheses wrap the value
		for ((this.attrs.count = 0); this.attrs.count < 1; (this.attrs.count += 1)) {
			print(a);
		}
		return [a, b, c];
	}
}
