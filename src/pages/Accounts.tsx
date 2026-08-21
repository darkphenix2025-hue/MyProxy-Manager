import CodexOAuthPanel from "../components/accounts/CodexOAuthPanel";

function Accounts() {
  return (
    <div className="h-full min-h-0 overflow-y-auto">
      <div className="mx-auto w-full max-w-7xl p-5">
        <CodexOAuthPanel />
      </div>
    </div>
  );
}

export default Accounts;
