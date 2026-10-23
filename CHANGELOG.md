# Changelog

## [0.3.0](https://github.com/jkorinth/rusthour/compare/v0.2.0...v0.3.0) (2026-10-23)


### ⚠ BREAKING CHANGES

* **api:** `Status::state_hash` is removed (use `Simulation::state_hash`), `Simulation` has two new required methods, and `PROTOCOL_VERSION` is 2.
* **api:** NETWORK_SCHEMA_VERSION is 3 and Road has a new roundabout field; .rhnet maps of schema 2 must be rebuilt.

### Features

* **api:** Pacer for real-time stepping ([5dc8ca7](https://github.com/jkorinth/rusthour/commit/5dc8ca7d97feca90e4cb31cb6d956f40d6e339b2))
* **api:** road headings and roundabout flag, network schema 3 ([0999138](https://github.com/jkorinth/rusthour/commit/09991386e558a23c41ad50a6c3143173282c99ac))
* **api:** stream vehicle snapshots and move the state hash out of Status ([07641d3](https://github.com/jkorinth/rusthour/commit/07641d3b79c0fb1698ebc902d782f890bc324e48))
* **api:** tell whether a road has a twin, so frontends draw it shifted ([9572eef](https://github.com/jkorinth/rusthour/commit/9572eef6c940f55909a6f31807239c4ceb0e5df3))
* **api:** vehicle snapshot types and Placement ([e23975e](https://github.com/jkorinth/rusthour/commit/e23975e0ccf756735b9b751e4c06feff83067cd9))
* **cli:** run on a map with --map and --vehicles ([ca40c7d](https://github.com/jkorinth/rusthour/commit/ca40c7d410737f19ed522e421f0c6d9a0bb823b4))
* **core:** A* routing and random origin–destination demand ([7cdf529](https://github.com/jkorinth/rusthour/commit/7cdf529a7de7ac43df13508a9b62f070c50a0915))
* **core:** IDM car-following acceleration ([b6700ee](https://github.com/jkorinth/rusthour/commit/b6700ee4d35db717a85161592c7e1c29cc542c65))
* **core:** one random stream per subsystem ([bd99e8d](https://github.com/jkorinth/rusthour/commit/bd99e8de9e8759e20e9c8f31d23b788d70278b70))
* **core:** priority rule for intersections and roundabouts ([cde4f84](https://github.com/jkorinth/rusthour/commit/cde4f846991d16bce93d0ebb3116231a83c93e64))
* **core:** spawn random demand trips through the router each step ([ad39137](https://github.com/jkorinth/rusthour/commit/ad39137a3e98b92ddc810eff60b24d09abaac596))
* **core:** step pipeline with IDM, integration, road transfers and arrivals ([23ffbed](https://github.com/jkorinth/rusthour/commit/23ffbeda264457ed86a2e056a74c27c0aa540751))
* **core:** vehicle store with generational slots and per-lane sorted lists ([a30fe13](https://github.com/jkorinth/rusthour/commit/a30fe13e283a0f4b3f26bccbf836856fcc4bce35))
* **core:** yield at intersections and stop at the line when told to ([e74936c](https://github.com/jkorinth/rusthour/commit/e74936cb38ae19d84dc182f5d52cd3465a7d14f1))
* **import:** set roundabout from junction=roundabout|circular ([dc466bd](https://github.com/jkorinth/rusthour/commit/dc466bdb52cf7f96c7b996220bb0c91f246821b3))
* **viz:** animate vehicles with play, pause, step and a speed slider ([8358007](https://github.com/jkorinth/rusthour/commit/8358007439ec2b4c81f31418ffa5c3f71f6848c6))


### Bug Fixes

* **core:** document IDM gap and desired-speed contract, test gap monotonicity ([68da855](https://github.com/jkorinth/rusthour/commit/68da855040805c6ff065c5986827674254564a90))
* **core:** give the vehicle test roads the new heading fields ([cf9f081](https://github.com/jkorinth/rusthour/commit/cf9f0816a07626ffe17fffc4deaf1c5dc2cecacf))


### Performance Improvements

* **core:** add criterion benchmarks for stepping and routing ([2fc5ead](https://github.com/jkorinth/rusthour/commit/2fc5ead1e157da824b0b7f30081684e8d62d4d3e))
* **core:** pre-reserve each road's pending-trip queue ([cbfdc29](https://github.com/jkorinth/rusthour/commit/cbfdc29585767d2284393c927cd7a72d17211da4))

## [0.2.0](https://github.com/jkorinth/rusthour/compare/v0.1.0...v0.2.0) (2026-10-22)


### Features

* **api:** add network data types, validated loading and a geometry query ([474406e](https://github.com/jkorinth/rusthour/commit/474406e7b0be5f6f712772725767c07e7a515666))
* **api:** add OSM source references and street names to network items ([5b356c5](https://github.com/jkorinth/rusthour/commit/5b356c597ea6fa9e3b3d27f18e7f62583789266f))
* **core:** add a static road network and include it in the state hash ([ff5842a](https://github.com/jkorinth/rusthour/commit/ff5842a2dc58bf9c96809c1943aa8cbfa3393a4a))
* **data:** build .rhnet maps in the assets workflow ([8094ef0](https://github.com/jkorinth/rusthour/commit/8094ef0b9783f9b463db54b0f177685c6866884f))
* **data:** publish sample map extracts as assets-* pre-releases ([4085fb2](https://github.com/jkorinth/rusthour/commit/4085fb270b15b7d8588a2c79066ca3276e753fd9))
* **import:** add the .rhnet map asset format ([4c76b12](https://github.com/jkorinth/rusthour/commit/4c76b12fbd97bd762a9d74316dfb07411b687bfb))
* **import:** drop road islands not connected to the area's center ([af7521c](https://github.com/jkorinth/rusthour/commit/af7521c0e6e961b2e45019b49a1403ae2bd2d04b))
* **import:** import OSM extracts into networks with a rusthour-import binary ([723403d](https://github.com/jkorinth/rusthour/commit/723403d291916f9cfbb4d23603265cabff44bdd5))
* **viz:** frame-time and startup overlay ([a1027e0](https://github.com/jkorinth/rusthour/commit/a1027e066c893440940e85880cfd75bbbae2cdb0))
* **viz:** load .rhnet maps with progress and errors ([28ea565](https://github.com/jkorinth/rusthour/commit/28ea5658d4470ae5edbbda59ea8ec500bb95c656))
* **viz:** pan with middle button and Shift-drag ([131ae48](https://github.com/jkorinth/rusthour/commit/131ae480d767dda869a5095c5298d9d030db5bc3))
* **viz:** render the road network with pan and zoom ([539b40a](https://github.com/jkorinth/rusthour/commit/539b40acb056a97c77b7c11652b667d84b887d76))
* **viz:** street-name labels ([ac2e530](https://github.com/jkorinth/rusthour/commit/ac2e530d2777fc9b157e16eb9bd2c846c35ea96e))
* **viz:** switch maps from a selector listing the site's maps ([2d28ed2](https://github.com/jkorinth/rusthour/commit/2d28ed25ab743353e036e6636c189ec29749822d))


### Bug Fixes

* **ci:** keep the OSM data's date in the cut area extracts ([36ba86f](https://github.com/jkorinth/rusthour/commit/36ba86fa03cbce044c10469206e4cfe3e3044fea))
* **ci:** pass ?map= through the site's version page ([6857e66](https://github.com/jkorinth/rusthour/commit/6857e6657bd8273f5107041f4727c394a48b7870))
* **viz:** hide an unknown map data date ([827b4df](https://github.com/jkorinth/rusthour/commit/827b4df26b7461737224a4611ed404bed8318d70))
* **viz:** stop the viewer freezing on the first map drag ([3796cc8](https://github.com/jkorinth/rusthour/commit/3796cc8a3e4d390dcbe628c4aaef81c709d0e8dd))
* **viz:** zoom faster, fit around the overlays and keep UI-zoom keys free ([6fcd736](https://github.com/jkorinth/rusthour/commit/6fcd736486b5478f6e2e2f5c8c5ed7ff66a2c1b4))

## 0.1.0 (2026-10-21)


### Features

* scaffold workspace with deterministic core engine ([36926ac](https://github.com/jkorinth/rusthour/commit/36926ac29cbfd6d6e74ad00c0c175f91ec4e4dd5))
