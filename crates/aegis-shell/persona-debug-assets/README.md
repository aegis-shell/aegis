# Local VRM Debug Assets

This directory keeps developer-selected VRM and VRMA binaries out of Git.
Use `AEGIS_AVATAR_DEBUG_ASSETS=1` to opt into them.

The current local `avatar.vrma` is derived from CMU Graphics Lab motion
`141_16`, described as “Wave Hello.” The BVH copy came from
[`una-dinosauria/cmu-mocap`](https://github.com/una-dinosauria/cmu-mocap/blob/09a07f54f3bbb58797325f009282d0b2048a2871/data/141/141_16.bvh)
and was converted with VRM Consortium
[`bvh2vrma`](https://github.com/vrm-c/bvh2vrma/tree/da148d9a377739cef91c1a1e57d56d381a88aadc).
The root orientation was normalized by 180 degrees around Y so a conforming
VRM faces its portrait camera.

- Source BVH SHA-256:
  `d428d2c4fa8873d077537567ad32c95b9687a479a10522d314b203fddea37daf`
- Local VRMA SHA-256:
  `ec03065a9a5e5bba27ee8171c756dc9caa34471aa34c54cb60ceee1c93208207`
- Duration: `2.4916568` seconds

The [CMU Motion Capture Database](https://mocap.cs.cmu.edu/) permits use of
the data but prohibits directly reselling it, including converted data. Keep
the generated VRMA local and acknowledge the CMU database in published work.
