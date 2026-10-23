//! Independent work over the machine's cores, with scoped threads and results in input order.

/// The most threads started, and the fewest items worth giving one.
const MAX_THREADS: usize = 8;
const MIN_ITEMS_PER_THREAD: usize = 32;

/// `work` applied to each of `items`, in order. A short list runs on the calling thread. A panic in `work` is
/// raised again here.
pub(crate) fn map_in_parallel<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(MAX_THREADS);
    let per_thread = items.len().div_ceil(threads).max(MIN_ITEMS_PER_THREAD);
    if items.len() <= per_thread {
        return items.iter().map(&work).collect();
    }
    std::thread::scope(|scope| {
        let workers: Vec<_> = items
            .chunks(per_thread)
            .map(|chunk| scope.spawn(|| chunk.iter().map(&work).collect::<Vec<R>>()))
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| match worker.join() {
                Ok(results) => results,
                Err(panic) => std::panic::resume_unwind(panic),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::map_in_parallel;

    #[test]
    fn results_keep_the_order_of_the_items() {
        let items: Vec<usize> = (0..1000).collect();
        assert_eq!(
            map_in_parallel(&items, |item| item * 2),
            items.iter().map(|item| item * 2).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_short_list_and_an_empty_one_are_mapped_too() {
        assert_eq!(map_in_parallel(&[1, 2, 3], |item| item + 1), vec![2, 3, 4]);
        assert!(map_in_parallel(&[] as &[u8], |item| *item).is_empty());
    }

    #[test]
    fn a_panic_in_the_work_is_raised_again() {
        let items: Vec<usize> = (0..1000).collect();
        let outcome = std::panic::catch_unwind(|| map_in_parallel(&items, |item| assert_ne!(*item, 700)));
        assert!(outcome.is_err());
    }
}
