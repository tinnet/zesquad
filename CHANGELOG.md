# Changelog

What changed in each zesquad release. Versions are dates (CalVer).

## 2026.11.25.8

### Features

- **web**: Fold how auction prices are collected away by default ([5f7e601](https://github.com/tinnet/zesquad/commit/5f7e601bcc5911f0f42eba53bd186034f70eef69))

## 2026.11.25.7

### Performance

- **web**: Speed up the auction overview and tidy today's code ([77221d1](https://github.com/tinnet/zesquad/commit/77221d14cd45f11ff2e0b668d59a53ca1aab762f))

## 2026.11.25.6

### Features

- **web**: Show a market overview on the auction house page ([1341420](https://github.com/tinnet/zesquad/commit/1341420c091bb23a956253674f04e904e8b6b7e0))

## 2026.11.25.5

### Fixes

- **web**: Drop empty coins and fit the auction table on the page ([5102cd6](https://github.com/tinnet/zesquad/commit/5102cd633ba889fb1fed41591786a7927b3909cb))

## 2026.11.25.4

### Features

- **web**: Show what a vendor pays under each auction item ([e38a5d6](https://github.com/tinnet/zesquad/commit/e38a5d65470a828c942c79c540198c0f8775dcbf))

## 2026.11.25.3

### Features

- **web**: Show item icons on the auction house page ([ffa0f09](https://github.com/tinnet/zesquad/commit/ffa0f09ffe1ad7e862ca52eb8de0ece3581f0329))

## 2026.11.25.2

### Features

- **web**: Search items by name on the auction house page ([022d240](https://github.com/tinnet/zesquad/commit/022d240a44a734519d77053db95f073dff20ce86))

## 2026.11.25.1

### Features

- **armory**: Look up names and details of items seen at auction ([dc62932](https://github.com/tinnet/zesquad/commit/dc6293241eeacb6b3b058a79a5053b1b72160241))

### Fixes

- **armory**: Wait as long as Battle.net asks after a 429 ([cca2587](https://github.com/tinnet/zesquad/commit/cca25872f8efc55c159d3d52bdc2c5e2080c1dac))

## 2026.11.25

### Features

- **web**: Replace the Z favicon with a squad of three figures ([3cdf42e](https://github.com/tinnet/zesquad/commit/3cdf42e99d9b521d5336958ce53ddbbe802c0075))

## 2026.11.24

### Features

- Add flavour lines to Discord join and level-up messages ([5648675](https://github.com/tinnet/zesquad/commit/56486756ea0460b61a507243587fe39048d487f5))
- More variety in Discord flavour lines ([b82edcb](https://github.com/tinnet/zesquad/commit/b82edcb4ab61e0e757b33995b04e328ca1ea05c4))

## 2026.11.23.3

### Features

- **web**: Show each character's ruleset on its card ([5d9f327](https://github.com/tinnet/zesquad/commit/5d9f327afd3c28e17e2180b5683d420bfa0ea64e))

## 2026.11.23.2

### Features

- **web**: Guild pages with a searchable roster ([051cbe8](https://github.com/tinnet/zesquad/commit/051cbe8b84acffdf1ae76c76905efc1349d0a5fe))

### Documentation

- Guild rosters come whole, without paging ([23fef0f](https://github.com/tinnet/zesquad/commit/23fef0fe4810910567b9f905cf5c43814c1cc90b))

## 2026.11.23.1

### Features

- **web**: Move squad settings to their own page ([0af68fb](https://github.com/tinnet/zesquad/commit/0af68fbb03b93650c1297ff56acc912bc8cc6f3d))

## 2026.11.23

### Features

- **web**: Slow down repeated logins and changes ([72e28e3](https://github.com/tinnet/zesquad/commit/72e28e31888e6941b75b55bf058afc1488f7a032))

### Fixes

- **web**: A removed member can't rejoin with the old invite link ([6993168](https://github.com/tinnet/zesquad/commit/6993168c2b9985778aa9d27b55dc3ad15ffff78a))
- **web**: Losing the race to claim an old squad grants nothing ([f82ca99](https://github.com/tinnet/zesquad/commit/f82ca99d22efced0ab2e3274ddb572404f564b53))
- **web**: Turning logins off no longer exposes private squads ([f6672f9](https://github.com/tinnet/zesquad/commit/f6672f92ea55bd2a6008b7677d47f5b25ea6afea))

## 2026.11.22.4

### Features

- **db**: Squads have an owner and members ([2310975](https://github.com/tinnet/zesquad/commit/2310975abfefa012a7b92aad405235db748d64e2))
- **web**: Only members change a squad ([e7761c7](https://github.com/tinnet/zesquad/commit/e7761c75e3e6c9278318298501f0d70c432e96de))
- **web**: Invite friends with a link ([be1a4c8](https://github.com/tinnet/zesquad/commit/be1a4c832c47c9f76fcae6643ccba25557602120))
- **web**: Squads from before accounts go to whoever opens them first ([30296a1](https://github.com/tinnet/zesquad/commit/30296a147ed42b8fbd24d66b1b68ba92c83223f6))
- **web**: Choose who can see a squad ([7c19d90](https://github.com/tinnet/zesquad/commit/7c19d902da7778744ce61bb707673bb09ccb1c83))
- **web**: Your squads on the front page ([e6bb1fe](https://github.com/tinnet/zesquad/commit/e6bb1fe530d707682f6d577bfef108a60f29a158))
- **web**: Owners manage members and can delete a squad ([f06cc44](https://github.com/tinnet/zesquad/commit/f06cc44d6e9421e2f361a21eaac69fe2640b9646))
- **web**: Browse public squads ([1d09156](https://github.com/tinnet/zesquad/commit/1d09156812f2827a8e6e9b58604855dcb69f8cd4))

### Fixes

- **db**: Tagging a character while it's being looked up no longer fails ([6012f87](https://github.com/tinnet/zesquad/commit/6012f87cc6a2f393a988e9e3b4ebef9c2aa8f4f8))

## 2026.11.22.3

### Features

- **web**: Post new characters and level-ups to a Discord channel ([b61f6ba](https://github.com/tinnet/zesquad/commit/b61f6bac81e33fd93df2ae7a267e727aca5f5e4d))

## 2026.11.22.2

### Features

- Keep every raw auction dump, optionally in an S3 bucket ([2f0c50e](https://github.com/tinnet/zesquad/commit/2f0c50e10153ec6c501a4ba9175e0ae0c7cf78e4))

## 2026.11.22.1

### Features

- **web**: Log in with Battle.net ([4d26fe5](https://github.com/tinnet/zesquad/commit/4d26fe5ab22ff1b810ac12dd9b4de5f8434e3109))
- **web**: See and delete what zesquad keeps about you ([7f0be82](https://github.com/tinnet/zesquad/commit/7f0be8224242b3733c4653e959f86ab9ea3f88f4))

## 2026.11.22

### Features

- **web**: Sort a squad by name, level, item level or last played ([953e00f](https://github.com/tinnet/zesquad/commit/953e00f8e6bc8b0e18e667a302ac65fa238c0b95))
- **web**: Show newcomers how a squad works on the front page ([1dc61ff](https://github.com/tinnet/zesquad/commit/1dc61ff6fc14cc02a3abe940c629f47a2fb4b46f))

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


