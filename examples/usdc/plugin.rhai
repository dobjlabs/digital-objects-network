// A permissioned currency without a signature primitive: the mint/burn gate
// is consuming-and-reproducing the IssuerCap, so authority is knowing the
// cap's key, replay is structurally impossible (each use nullifies it), and
// the supply counter lives on the very object every issuance must touch.
//
// The one thing the proof CANNOT check is that a burned bill carries this
// cap's own stamp (there is no equality constraint over id-sized values), so
// Burn trusts its operator to feed it their own currency -- the same party
// the counter belongs to. Bills of a foreign stamp are a foreign currency:
// holders verify `issuer` against the published id, and Halve propagates the
// stamp so change stays genuine.

fn rekey(action, obj) {
    var key = action.random();
    obj.update("key", key);
}

fn Genesis(action) {
    var cap = action.output("IssuerCap");
    var issuer_id = action.random();
    cap.set([
        ["issuer_id", issuer_id],
        ["total_issued", 0]
    ]);
}

fn RekeyIssuerCap(action) {
    var cap = action.mutate("IssuerCap");
    rekey(action, cap);
}

fn mint(action, amount) {
    var cap = action.mutate("IssuerCap");
    var total = unsafe { cap.total_issued + amount };
    action.st_sum(cap.total_issued, amount, total);
    cap.update("total_issued", total);
    var bill = action.output("USDC");
    bill.set([
        ["amount", amount],
        ["issuer", cap.issuer_id]
    ]);
}

fn Mint1024(action) {
    mint(action, 1024);
}

fn Mint1(action) {
    mint(action, 1);
}

fn Halve(action) {
    var coin = action.input("USDC");
    var a = action.output("USDC");
    var b = action.output("USDC");
    var half = unsafe { coin.amount / 2 };
    action.st_sum(half, half, coin.amount);
    a.set([
        ["amount", half],
        ["issuer", coin.issuer]
    ]);
    b.set([
        ["amount", half],
        ["issuer", coin.issuer]
    ]);
}

fn RekeyUSDC(action) {
    var bill = action.mutate("USDC");
    rekey(action, bill);
}

fn Burn(action) {
    var cap = action.mutate("IssuerCap");
    var coin = action.input("USDC");
    var total = unsafe { cap.total_issued - coin.amount };
    action.st_sum(total, coin.amount, cap.total_issued);
    cap.update("total_issued", total);
}
