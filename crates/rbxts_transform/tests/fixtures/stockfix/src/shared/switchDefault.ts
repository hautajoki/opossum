// A `default` that is not the last clause. roblox-ts lowered it as the end of the switch: every case after it was
// dropped, so it ran the default where JavaScript runs a later case.

export function middle(x: number) {
	const log: string[] = [];
	switch (x) {
		case 1:
			log.push("one");
		default:
			log.push("default");
		case 2:
			log.push("two");
			break;
		case 3:
			log.push("three");
	}
	return log;
}

export function first(x: number) {
	const log: string[] = [];
	switch (x) {
		default:
			log.push("default");
			break;
		case 1:
		case 2:
			log.push("one or two");
	}
	return log;
}

// later cases are tested in order, once each, and only until one matches
export function testedOnce(x: number) {
	const log: string[] = [];
	const test = (value: number) => {
		log.push(`test ${value}`);
		return value;
	};
	switch (x) {
		case test(1):
			log.push("one");
			break;
		default:
			log.push("default");
			break;
		case test(2):
			log.push("two");
			break;
		case (log.push("prereq"), 3):
			log.push("three");
	}
	return log;
}
