export function clamp(value: number, min: number, max: number) {
	return math.clamp(value, min, max);
}

export class Vector {
	public constructor(
		public readonly x: number,
		public readonly y: number,
	) {}

	public add(other: Vector) {
		return new Vector(this.x + other.x, this.y + other.y);
	}
}
