fn main() {
    let repo = gix::discover(".").unwrap();
    let head = repo.head_commit().unwrap();
    println!("HEAD: {}", head.id);
}
