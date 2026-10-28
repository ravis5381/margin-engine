use criterion::{Criterion, black_box, criterion_group, criterion_main};
use margin_domain::{Currency, Money};

fn money_checked_add(c: &mut Criterion) {
    let left = Money::parse("123456789.123456", Currency::Inr).expect("left amount");
    let right = Money::parse("0.000001", Currency::Inr).expect("right amount");
    c.bench_function("money_checked_add", |bencher| {
        bencher.iter(|| {
            black_box(left)
                .checked_add(black_box(right))
                .expect("addition stays in range")
        })
    });
}

criterion_group!(benches, money_checked_add);
criterion_main!(benches);
