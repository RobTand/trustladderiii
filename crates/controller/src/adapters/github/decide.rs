/// State-table Comment/message contract: a visible marker prevents another post.
pub(super) fn should_post(_found: bool) -> bool {
    false
}
