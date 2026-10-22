# Changelog

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
