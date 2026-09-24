#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: test_allocation_counter::CountingAllocator = test_allocation_counter::CountingAllocator;
