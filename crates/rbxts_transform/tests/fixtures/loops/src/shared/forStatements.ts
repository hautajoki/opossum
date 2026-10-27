// for statements: the --optimizedLoops numeric for, and the while-loop fallback

export function numericLoops(n: number) {
	const COUNT = 10;
	const STEP = 2;
	const NEGATIVE = -3;
	for (let i = 0; i < 10; i++) {
		print(i);
	}
	for (let i = 1; i <= COUNT; i += 2) {
		print(i);
	}
	for (let i = 10; i > 0; i--) {
		print(i);
	}
	for (let i = 10; i >= -5; i -= 3) {
		print(i);
	}
	for (let i = 0; i < COUNT + 1; i = i + STEP) {
		print(i);
	}
	for (let i = 0; COUNT > i; i = STEP + i) {
		print(i);
	}
	for (let i = COUNT; i > 0; i = i - STEP) {
		print(i);
	}
	for (let i = 0; i < COUNT; i += STEP) {
		print(i);
	}
	for (let i = COUNT; i >= 0; i -= STEP) {
		print(i);
	}
	for (let i = 0; i <= 100; i += 2 * 3) {
		print(i);
	}
	for (let i = 100; i >= 0; i -= 2 * 3) {
		print(i);
	}
	for (let i = 0; i > -20; i += NEGATIVE) {
		print(i);
	}
	for (let i = 0; (i) < (COUNT - 1); (i)++) {
		print(i);
	}
	for (let i = 0; i < n; i++) {
		print(i);
	}
}

export function fallbackLoops(n: number) {
	// the body writes the variable
	for (let i = 0; i < 10; i++) {
		if (i === 3) {
			i += 2;
		}
		print(i);
	}
	// a wrong-way step
	for (let i = 0; i < 10; i--) {
		print(i);
		break;
	}
	// a zero step
	for (let i = 0; i < 10; i += 0) {
		print(i);
		break;
	}
	// not an integer
	for (let i = 0.5; i < 10; i++) {
		print(i);
	}
	// an unsupported comparison
	for (let i = 0; i !== 10; i++) {
		print(i);
	}
	// two variables
	for (let i = 0, j = 10; i < j; i++, j--) {
		print(i, j);
	}
	// no initializer, no incrementor
	let k = 0;
	for (; k < n; ) {
		k++;
	}
	// an expression initializer
	for (k = 0; k < n; k++) {
		print(k);
	}
	// no condition
	for (let i = 0; ; i++) {
		if (i > n) break;
	}
	// nothing at all
	for (;;) {
		if (n > 0) break;
	}
	// a const variable
	for (const value = n; value < 10; ) {
		print(value);
		break;
	}
}

export function capturedLoops(n: number) {
	const callbacks = new Array<() => void>();
	// read from a function: a fresh copy per iteration
	for (let i = 0; i < n; i++) {
		callbacks.push(() => print(i));
	}
	// with continue, the copy is written back before every continue
	for (let i = 0; i < n; i++) {
		if (i % 2 === 0) {
			continue;
		} else if (i % 3 === 0) {
			print("three");
			continue;
		} else {
			callbacks.push(() => print(i));
		}
		do {
			if (i > 5) continue;
		} while (false);
	}
	// used in its own initializer: a copy of the copy
	for (let i = n, j = i; i < 10; i++) {
		callbacks.push(() => print(i, j));
	}
	// written in the body
	for (let i = 0; i < n; i++) {
		i++;
		if (i > 4) {
			return;
		}
	}
	// a step read from a function is not known to be initialized
	const run = () => {
		for (let i = 0; i < 10; i += STEP_LATER) {
			print(i);
		}
	};
	const STEP_LATER = 2;
	run();
	return callbacks;
}

export function loopConditions(values: Array<number>, text: string, flag?: boolean) {
	for (let i = 0; i < values.size() && values[i] !== 0; i++) {
		print(values[i]);
	}
	for (let i = 0; text; i++) {
		text = text.sub(2);
	}
	for (let i = 0; flag ?? i < 3; i++) {
		print(i);
	}
	for (let i = 0; i < 10; i++) print(i);
}
