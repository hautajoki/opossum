// a seam temporary made first: the name is not approved afterwards
export function h() {
	const c = probeHoist(4);
	return c + probeApprove(5);
}
