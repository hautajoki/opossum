/// <reference types="types" preserve="true" />
import { Players } from "@rbxts/services";
import { clamp, Vector } from "util/math";
import type { Options } from "shapes";
import { native } from "native";

export * from "util/math";
export { describe } from "./util/describe";
export type { Shape } from "shapes";

export function configure(options: Partial<Options>): Options {
	return { name: options.name ?? "default", size: options.size ?? native(1) };
}

export function origin() {
	return new Vector(0, 0);
}

export function playerCount() {
	return Players.GetPlayers().size();
}

/** A counter with a limit. */
export class Counter {
	private value = 0;

	public constructor(private readonly limit: number) {}

	public increment(by = 1) {
		this.value = clamp(this.value + by, 0, this.limit);
		return this.value;
	}

	public current() {
		return this.value;
	}
}

export const enum Mode {
	Fast,
	Slow = "slow",
}

export type Loader = typeof import("util/math");

export default Counter;
