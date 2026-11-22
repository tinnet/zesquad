# Changelog

What changed in each zesquad release. Versions are dates (CalVer).

## 2026.11.21

### Features

- **web**: List the squads you opened on the front page ([f87e9a5](https://github.com/tinnet/zesquad/commit/f87e9a5b61462664956da43aa2a63c7d11d20e37))
- **web**: Copy a squad's link in one click ([e9c5e48](https://github.com/tinnet/zesquad/commit/e9c5e4864cd470d743d0d24e2006c859cc569cd7))
- **web**: Show the squad's name when its link is shared ([5446b85](https://github.com/tinnet/zesquad/commit/5446b85d6fceda7a68ef1f083cd577c2b0a411c5))

## 2026.11.20.3

### Performance

- **armory**: Ask Battle.net whether auction dumps changed before downloading them ([16bd049](https://github.com/tinnet/zesquad/commit/16bd0498317537b85f09bbdb581764f27eb8fda5))

## 2026.11.20.2

### Features

- **web**: Show auction prices with coins and a line of the last week ([036b619](https://github.com/tinnet/zesquad/commit/036b61986b9dc03f644ba0b10d7891e4310a18f1))

## 2026.11.20.1

### Features

- **web**: Show auction house collection and leather prices ([049e1f6](https://github.com/tinnet/zesquad/commit/049e1f643c5872c89f89bbc7741798d89eee3d0b))

## 2026.11.20

### Features

- **armory**: Follow auction house prices in EU and US ([defdb82](https://github.com/tinnet/zesquad/commit/defdb82030e1b54729b583276fc047bc69eaf77d))

## 2026.11.19

### Features

- **web**: Pick a region per squad ([525e1ba](https://github.com/tinnet/zesquad/commit/525e1ba64e23065e9d3fa416ca5dea357c61ec12))

## 2026.11.18.2

### Features

- **web**: Shorter squad links without dashes ([7802995](https://github.com/tinnet/zesquad/commit/780299595fc14efeff7a101a44787de9c5705cd3))

## 2026.11.18.1

### Fixes

- Show the release version in --version, not 0.1.0 ([5786e93](https://github.com/tinnet/zesquad/commit/5786e938731ad5bedb8ed3736ea2b95cf64a38ec))

## 2026.11.18

### Features

- **core**: Parse character names, tags and Armory responses ([058e095](https://github.com/tinnet/zesquad/commit/058e095595dcd2005a26eab4ad1ffcb25611f852))
- Friend squads with Armory details for each character ([64db3f8](https://github.com/tinnet/zesquad/commit/64db3f8233a72594adcd41235ebb6b0d55ad64a6))
- Optional shared password in front of the whole site ([18e71e4](https://github.com/tinnet/zesquad/commit/18e71e413ce2c9752920c97b5d93d778c353acd2))
- **web**: Tag characters by class, spec, race, faction and professions, and colour tags ([94771aa](https://github.com/tinnet/zesquad/commit/94771aac4b362eb2ba26e07439ffd8ad85dca492))

### Fixes

- Let the container create its database on a new volume ([65a22e5](https://github.com/tinnet/zesquad/commit/65a22e5e6db956f2436e8b69841c5f30fa1111b4))
- **web**: Stop the login page redirecting to other sites ([efa833c](https://github.com/tinnet/zesquad/commit/efa833c44ddb51950d2b5f56a42a9cdc36ab4fc8))
- **web**: After logging in, return to the page you were on ([9fdf62e](https://github.com/tinnet/zesquad/commit/9fdf62e2dd679fd4df24236f254911ef34351cec))
- **web**: Keep a character's details while refreshing or on a failed lookup ([35572ba](https://github.com/tinnet/zesquad/commit/35572baa3cc8dc3af9de20bcf0c061213db3e32e))
- **web**: Stop the roster refresh wiping what you're typing ([8af8159](https://github.com/tinnet/zesquad/commit/8af815999fd06a3ea735a9196f6540c5152a1220))
- **armory**: Say so when Battle.net rejects a freshly fetched token ([816d050](https://github.com/tinnet/zesquad/commit/816d0507b6aa1535a3fafbb425b1645f432d9549))

### Performance

- **armory**: Reuse short-lived Battle.net tokens ([f9dbac6](https://github.com/tinnet/zesquad/commit/f9dbac6c61a94cd98620344189a92be2c17a6b6b))
- **db**: Parse each character's Armory documents once, not every render ([ef22c6b](https://github.com/tinnet/zesquad/commit/ef22c6b0f0f545036b5314f7b6cd1e70c74b0098))

### Documentation

- Describe what the Battle.net API offers for WoW: Forever ([6a12247](https://github.com/tinnet/zesquad/commit/6a122477d98698a035f9fa0b24b3b81fbc875d71))
- Explain running, configuring and developing squadron ([03e16cb](https://github.com/tinnet/zesquad/commit/03e16cb6eb32db8edb990b8baba7ab4b1b6635e9))
- Describe the Armory parse cache and token renewal ([dadc68d](https://github.com/tinnet/zesquad/commit/dadc68da1420a4656e7aa07bf96f9cdaa5cf32da))


