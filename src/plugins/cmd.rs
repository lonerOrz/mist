use crate::domain::Item;

pub fn query(args: &str) -> Vec<Item> {
    if args.is_empty() {
        return Vec::new();
    }
    vec![Item::new_command(args)]
}
