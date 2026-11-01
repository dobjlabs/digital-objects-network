fn ClaimFortuna(action) {
    var fortuna = action.output("Fortuna");
    action.st_equal(fortuna.key, 0);
}

fn BurnFortuna(action) {
    var fortuna = action.input("Fortuna");
}
