// `continue` inside a switch continues the loop around it. roblox-ts lowers a switch to a `repeat … until true`
// block and wrote `continue` as it was: that continued the block, leaving the switch like `break`, so the rest of the
// loop body still ran.

export function forOf() {
	const log: string[] = [];
	for (const x of [1, 2, 3]) {
		switch (x) {
			case 2:
				continue;
			default:
				log.push(`case ${x}`);
		}
		log.push(`after ${x}`);
	}
	return log;
}

// a `default` before other cases (the other lowering)
export function middleDefault() {
	const log: string[] = [];
	for (const x of [1, 2, 3]) {
		switch (x) {
			case 1:
				log.push("one");
				break;
			default:
				continue;
			case 3:
				log.push("three");
		}
		log.push(`after ${x}`);
	}
	return log;
}

// a switch inside a switch: the inner `continue` leaves both
export function nested() {
	const log: string[] = [];
	let i = 0;
	while (i < 4) {
		i++;
		switch (i % 2) {
			case 0:
				switch (i) {
					case 2:
						continue;
				}
				log.push(`even ${i}`);
				break;
			default:
				log.push(`odd ${i}`);
		}
		log.push(`after ${i}`);
	}
	return log;
}

// a loop inside a switch: its `continue` is that loop's, as it was
export function loopInSwitch() {
	const log: string[] = [];
	for (const x of [1, 2]) {
		switch (x) {
			case 1:
				for (const y of [1, 2, 3]) {
					if (y === 2) continue;
					log.push(`y ${y}`);
				}
				break;
			default:
				continue;
		}
		log.push(`after ${x}`);
	}
	return log;
}

// a `for` loop's increment still runs, and a `do … while` loop's condition is still checked
export function forAndDo() {
	const log: string[] = [];
	for (let i = 0; i < 4; i++) {
		switch (i % 2) {
			case 0:
				continue;
		}
		log.push(`for ${i}`);
	}
	let j = 0;
	do {
		j++;
		switch (j) {
			case 1:
				continue;
		}
		log.push(`do ${j}`);
	} while (j < 3);
	return log;
}

// a `continue` inside a try statement inside a switch (its function returns, and the switch passes it on)
export function tryInSwitch(values: number[]) {
	const log: string[] = [];
	for (const x of values) {
		switch (x) {
			case 2:
				try {
					continue;
				} finally {
					log.push(`finally ${x}`);
				}
		}
		log.push(`after ${x}`);
	}
	return log;
}

// a switch inside a try statement: the try's blocks are functions, so the switch returns `TS.TRY_CONTINUE` from them
// and the try statement continues the loop
export function switchInTry() {
	const log: string[] = [];
	for (const x of [1, 2, 3]) {
		try {
			switch (x) {
				case 2:
					continue;
			}
			log.push(`in ${x}`);
		} catch (e) {
			log.push("caught");
		}
		log.push(`after ${x}`);
	}
	return log;
}

// a try statement inside a switch inside a try statement
export function tryInSwitchInTry() {
	const log: string[] = [];
	for (const x of [1, 2, 3]) {
		try {
			switch (x) {
				case 2:
					try {
						continue;
					} finally {
						log.push(`fin ${x}`);
					}
			}
			log.push(`in ${x}`);
		} finally {
			log.push(`outer fin ${x}`);
		}
		log.push(`after ${x}`);
	}
	return log;
}

// a switch inside a try statement inside a switch
export function switchInTryInSwitch() {
	const log: string[] = [];
	for (const x of [1, 2, 3]) {
		switch (x) {
			case 2:
			case 3:
				try {
					switch (x) {
						case 2:
							continue;
					}
					log.push(`in ${x}`);
				} finally {
					log.push(`fin ${x}`);
				}
		}
		log.push(`after ${x}`);
	}
	return log;
}

// switch, try, switch, try: each level passes the `continue` on
export function deep() {
	const log: string[] = [];
	for (const x of [1, 2, 3]) {
		switch (x) {
			default:
				try {
					switch (x) {
						case 2:
							try {
								continue;
							} finally {
								log.push(`inner fin ${x}`);
							}
					}
				} finally {
					log.push(`fin ${x}`);
				}
		}
		log.push(`after ${x}`);
	}
	return log;
}
