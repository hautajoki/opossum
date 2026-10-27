// while and do-while statements

export function whiles(values: Array<number>, text: string, n: number, maybe?: number) {
	while (n > 0) {
		n--;
	}
	while (values.pop() !== undefined) {
		print(values.size());
	}
	while (text) {
		text = text.sub(2);
	}
	while (maybe ?? n) {
		maybe = undefined;
		n = 0;
	}
	while (true) {
		if (n++ > 10) break;
		if (n % 2 === 0) continue;
		print(n);
	}
	while (n < 20) n++;
	while (n > 100) {}
}

export function doWhiles(values: Array<number>, text: string, n: number) {
	do {
		n--;
	} while (n > 0);
	do {
		const x = n;
		print(x);
	} while (!(n > 5));
	do {
		print(values.size());
	} while (values.pop() !== undefined);
	do {
		text = text.sub(2);
	} while (text);
	do {
		if (n > 3) continue;
		n++;
	} while (n < 3);
	do n++;
	while (n < 30);
}
