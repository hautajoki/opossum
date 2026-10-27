@probeDecorator()
export class Decorated {
	static count = 1;
}

export class Plain {
	static count = 2;
}

export const widget = new ProbeWidget(5);
