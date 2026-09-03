use super::*;

#[test]
fn config_path_identity_is_absolute_and_lexically_normalized() {
    let relative = config_path_identity(Path::new("./config/../config.json"));
    let absolute = config_path_identity(&std::env::current_dir().unwrap().join("config.json"));
    assert_eq!(relative, absolute);
}
