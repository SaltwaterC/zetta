# age v1.3.1 post-quantum fixtures

Files written by the reference `age` implementation, so the ML-KEM-768 + X25519
recipient in `src/persistence/postquantum.rs` is checked against upstream on
every host, not only where `age` is installed. `src/tests/persistence/postquantum.rs`
reads them with `include_str!`/`include_bytes!`.

The identity is a throwaway test key generated for these fixtures. It protects
nothing.

Generated on 2026-10-14 with `age` v1.3.1 and `age-keygen` v1.3.1 (Homebrew
build, `age --version` prints `v1.3.1`), from this directory:

```sh
age-keygen -pq -o identity.txt
age-keygen -y -o recipient.txt identity.txt
printf 'zmux interoperability fixture: encrypted by age v1.3.1 to an ML-KEM-768 + X25519 recipient.\n' > plaintext.txt
age -r "$(cat recipient.txt)" -o ciphertext.age plaintext.txt
age -a -r "$(cat recipient.txt)" -o ciphertext.age.armored plaintext.txt
```

Both ciphertexts were checked with `age -d -i identity.txt` before they were
committed. `age` encrypts with a fresh file key and encapsulation each time, so
regenerating them changes every file; regenerate all of them together.
